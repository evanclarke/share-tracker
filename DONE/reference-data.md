# Done — Reference Data — Exchanges, Listings, Accounts & Holdings

## Reference Data — Exchange
- [x] Exchange model (MIC, name, country, currency, timezone, settlement period)
- [x] DB schema: `exchanges` table
- [x] Seed data for known exchanges (XASX, XNYS at minimum)
- [x] CRUD API endpoints for exchanges
- [x] Tests: insert, retrieve, upsert exchange

## Reference Data — Listing
- [x] Listing model (exchange FK, ticker, name, ISIN, security type, currency, AMIT flag)
- [x] DB schema: `listings` table
- [x] CRUD API endpoints for listings
- [x] Tests: insert, retrieve listing; FK constraint to exchange

## Accounts / ownership dimension
(REQUIREMENTS "Planned Enhancements — Accounts / ownership dimension". Everything is one flat portfolio today.)
- [x] NEEDS CLARIFICATION: decide whether to introduce an account/owner entity (Individual, Joint, SMSF, Family Trust) partitioning all holdings and reports per taxpayer — RESOLVED 2026-06-07: **out of scope**. Single taxpayer (individual resident); the custody/location split the user actually has is already covered by the holding-accounts feature. Revisit only if a second taxpayer appears (holding accounts would then belong to a taxpayer account, per REQUIREMENTS)
- [ ] If in scope: model the account entity (DB schema + migration); add an account FK to trades, income, AMMA statements, DRP enrolments; allow every report to be produced per account (FX/AUD rules unchanged within each) — N/A: decided out of scope (2026-06-07), see the resolution above
- [ ] Tests: gains and tax summaries are partitioned correctly across two accounts — N/A: decided out of scope (2026-06-07)
- [ ] README sync: account entity + per-account report parameters — N/A: decided out of scope (2026-06-07)

## Holding accounts — the same listing held in separate holdings
(REQUIREMENTS "Planned Enhancements — Holding accounts", added 2026-06-07. One implicit holding per listing today; the same listing held in two places — e.g. RSU-vested shares in an employer plan account that cannot DRP, plus DRP-enrolled shares in a personal broker account — is unrepresentable, as is the plan→personal transfer.)
- [x] Holding Account reference entity (unique Name): model, DB schema + migration seeding a default holding account, CRUD API endpoints (entity-module pattern), tests — `src/entities/holding_account.rs` (`HoldingAccount { id, name }`, `/holding_accounts` CRUD); migration `0016_holding_accounts.sql` creates `holding_accounts` (name UNIQUE) and seeds id 1 'Default' (`DEFAULT_HOLDING_ACCOUNT_ID`); `db_delete` refuses (`422`) an account still referenced by trades/income/AMMA/enrolments/transfers, or the seeded default itself; tests `holding_account::tests` (seed, CRUD round-trip, duplicate-name 422, referenced/default delete refused)
- [x] Migration: `trades`, `income`, `amma_statements`, and `drp_enrolments` gain a NOT NULL `holding_account_id` FK; existing rows migrate to the seeded default account (rename-pattern rebuild, no data dropped) — `0016_holding_accounts.sql` rebuilds the four tables (plus the dragged FK cluster: parcel_allocations, amit_adjustments, attachments) with `holding_account_id INTEGER NOT NULL DEFAULT 1 REFERENCES holding_accounts(id)`; also adds `trades.transfer_id` and the `transfers` table; `drp_enrolment::tests::migration_converts_old_enrolments_to_open_ended_periods` asserts old rows land in the default account
- [x] API writes that omit the holding account default to the seeded default account, so existing clients keep working — every body struct (`TradeBody`, `IncomeBody`, `AmmaStatementBody`, `DrpEnrolmentBody`, `SellBody`, `ExerciseBody`, `ParticipationBody`) carries `#[serde(default = holding_account::default_holding_account_id)]`; the entire pre-existing test suite passes unchanged through the default
- [x] DRP enrolment scoped per (listing, holding account): period-overlap/single-open invariants apply within one account; reinvestment eligibility tests the income record's account's enrolment as at the relevant date and the DRP trade lands in that account; the residual carry-forward chain runs per (listing, account) and never crosses accounts — `drp_enrolment::db_upsert` scopes the overlap check and the unenrolment payout to the period's account; `drp_reinvestment::db_reinvest` matches the enrolment on the income row's `holding_account_id`, stamps the DRP trade with it, and scopes the residual-brought-forward lookup to the same account; tests `drp_enrolment::tests::db_same_listing_may_be_enrolled_per_account`, `db_unenrolment_only_settles_the_periods_accounts_trades`, `drp_reinvestment::tests::enrolment_is_per_holding_account`, `drp_trade_lands_in_the_distributions_account`, `carried_residual_does_not_cross_accounts`
- [x] Write-time invariant: a Sell's parcel allocations may only consume Buy/DRP parcels in the Sell's holding account — reject with `422` — `sell::upsert_sell_in_tx` checks each parcel's account against `SellBody.holding_account_id` (`SellError::PurchaseInDifferentAccount` → `422`); scrip-for-scrip/demerger closing Sells are exempt (they mechanically close the whole holding across every account, their replacements staying per-account); tests `sell::tests::db_allocation_in_different_account_is_rejected`, `api_allocation_in_different_account_returns_422`
- [x] Write-time invariant: an AMIT adjustment may only target trades in its AMMA statement's holding account — reject with `422` — `amit_adjustment::db_upsert` compares the trade's and statement's accounts (`UpsertError::HoldingAccountMismatch` → `422`); test `amit_adjustment::tests::db_holding_account_mismatch_rejected`
- [x] Transfer between holding accounts — `src/entities/transfer.rs`: `PUT /transfers/:id` records (date, listing, from/to accounts — must differ — per-parcel quantities) and executes in one transaction: a price-0 transfer-out Sell in the source account through the shared sell core (stamped `transfer_id`) consumes the chosen quantity per parcel, and one transfer-in Buy per consumed parcel lands in the destination carrying the moved units' share of the remaining reduced cost base (AMIT/ROC-adjusted, pro-rated for partial, on the `brokerage` column), the parcel's currency/fx fallback, and its acquisition date as `deemed_acquisition_date`; returns `201` with the group. Not a CGT event: excluded from realised-gains/net-capital-gain (`transfer_id IS NULL` in `db_realised_gains`) and the franking at-risk walk. Group trades are immutable individually (PUT/DELETE /trades, PUT /sells, DELETE /sells all `422`); a recorded transfer is immutable (re-PUT `422`); `DELETE /transfers/:id` removes the group + record, restoring the pre-transfer holding (refused `422` while a transfer-in is drawn on)
- [x] Corporate-action operations stay within each parcel's holding account; rights exercise and buy-back participation say which account they act in — scrip-for-scrip and demerger replacement Buys inherit each consumed parcel's `holding_account_id`; `ExerciseBody`/`ParticipationBody` gain `holding_account_id` (default 1): the exercised parcel, the participation Sell, and its dividend income row land in it (participation allocations are bound by the same-account Sell invariant); splits/bonus issues re-base quantities per parcel so they are account-correct by construction; tests `scrip_exchange::tests::replacements_stay_in_each_parcels_holding_account`, `demerger::tests::replacements_stay_in_each_parcels_holding_account`, `rights_exercise::tests::exercise_lands_in_the_chosen_holding_account`, `buyback_participation::tests::participation_acts_in_the_chosen_holding_account`
- [x] Reports carry the holding account: portfolio, open parcels, and unrealised gains show the same listing once per account; taxpayer-level totals (tax summary, net capital gain) unchanged, rows identify the account — `portfolio::db_holdings` and `unrealised_gains::db_unrealised_gains` group per `(listing_id, holding_account_id)`; `open_parcels` rows carry `holding_account_id` (sorted listing → account → acquisition date); `realised_gains` rows carry the Sell's `holding_account_id`; tax summary / net capital gain aggregate across accounts unchanged (one taxpayer); test `portfolio::tests::db_same_listing_in_two_accounts_reports_as_two_holdings`
- [x] Web UI: holding accounts maintainable; trades, income, AMMA, DRP enrolments and transfers show/select the account — `holding_accounts` ENTITIES entry (Reference data), `holdingAccounts` select source, account field + column on the trades/income/AMMA/DRP-enrolment configs and the Sell form, account selects on the exercise/participate forms, and a bespoke Transfers view (list + create with per-parcel rows via `PUT /transfers/:id`; delete restores the pre-transfer holding); `web::tests::holding_account_ui_present`, `transfers_ui_present`
- [x] Tests: same listing in two accounts reports as two holdings; a distribution on the enrolled account reinvests while the unenrolled account's distribution is rejected; residual carry-forward never crosses accounts; a transfer preserves cost base and acquisition date and appears in no gains report; a partial transfer splits the parcel; a Sell consuming another account's parcel rejected with `422`; deleting a transfer restores the pre-transfer holdings — covered by the tests cited above plus `transfer::tests::transfer_moves_parcel_preserving_cost_base_and_acquisition_date`, `partial_transfer_splits_the_parcel`, `transfer_is_absent_from_gains_reports`, `deleting_the_transfer_restores_the_pre_transfer_holding`, `delete_is_refused_while_a_transfer_in_is_consumed`, `transfer_trades_are_immutable_individually`, `amit_and_roc_reductions_carry_into_the_transferred_cost_base`, `invalid_transfers_are_rejected_and_nothing_persisted`, `api_transfer_round_trip`, `api_invalid_transfer_returns_422`
- [x] README sync: `holding_accounts` + `transfers` schema, the new FK columns (+ Relationships), holding-account/transfer endpoints, response codes — schema blocks for both tables and every new column, Relationships lines, Holding accounts + Transfers HTTP API sections, holding-account notes on the Trades/Income/AMMA/DRP enrolments/DRP reinvestment/Sells/AMIT adjustments sections and the per-account report shapes, two Features bullets, the Web frontend list, and the `201`/`422` Response codes rows extended

## Ticker and exchange-code changes (REQUIREMENTS 2026-07-26)
A rename was already identity-safe (in-place `PUT /listings/:id`, everything keyed on
`listings.id` — parcels, cost bases, and the 12-month discount clock stay attached, pinned by
`reports::open_parcels::tests::db_ticker_rename_keeps_parcels_attached_to_the_listing` and
`reports::realised_gains::tests::db_sale_after_ticker_rename_keeps_cost_base_and_discount_clock`).
This section closed the price-fetch and presentation gaps around it: no provider-symbol escape
hatch, a wrong/dead symbol failing silently and permanently, historical documents relabelling
after a rename, and an unrecorded exchange change. LAAC → LAR was the prompting real-world case.
- [x] Migration `0018_listing_renames.sql`: new `listing_renames` table (`listing_id`,
      `effective_date`, `old_ticker`/`new_ticker`, `old_exchange_mic`/`new_exchange_mic`, `note`;
      `UNIQUE(listing_id, effective_date)`; CHECK rejecting a no-op rename) plus nullable
      `listings.price_symbol`. Deliberately no `*_stale_snapshots_*` trigger pair (a snapshot's
      ticker is a display label, never a computed figure). `listing_renames` joining the audited
      set required rebuilding `row_history` itself to extend its `table_name` CHECK (a table-level
      CHECK SQLite cannot `ALTER`) — hit and fixed two real SQLite gotchas along the way: (1) the
      `row_history_row` index from the pre-rename table still existed under its old name when the
      rename-pattern tried to recreate it (fixed by creating the index after dropping the `_old`
      table, not before); (2) the bundled SQLite's default `ALTER TABLE ... RENAME TO` rewrites
      every *other* trigger body that references the renamed table by name (even ones defined on
      unrelated tables, like every other audited table's own row-history trigger inserting into
      `row_history`) to point at the new name — silently breaking them the moment the `_old` table
      is dropped; fixed with `PRAGMA legacy_alter_table = ON` around the rename, confirmed by direct
      SQLite experimentation (a naive raw-sqlite3 repro didn't reproduce it — the two environments'
      SQLite versions disagree on this specific behaviour). Test:
      `reports::row_history::tests::audited_tables_match_migration_check_and_triggers` extended with
      a per-migration exception block (mirroring the existing attachments-rebuild precedent) plus a
      new `every_audited_table_records_update_and_delete` case; `docs/SCHEMA.md` updated (the new
      table/column, the Relationships section, the audited-tables prose)
- [x] `src/entities/listing_rename.rs`: `POST /listings/:id/rename` (records the event — with
      `old_ticker`/`old_exchange_mic` always read from the listing's current row, never trusted from
      the request, so the chain can't be falsified — and updates the listing, atomically; 422 on a
      no-op, an `effective_date` not after the listing's most recent rename, a ticker collision, or
      an unrecognised Crypto digital-token ticker), `GET /listings/:id/renames` (the chain, newest
      first), `DELETE /listings/:id/renames/:rename_id` (undo — restores `ticker`/`exchange_mic`
      from `old_*` — only for the newest rename in the chain; 422 otherwise). `listing::db_upsert`
      gained the restriction that makes the rename path mandatory: a bare `PUT` changing `ticker` or
      `exchange_mic` is refused (`UpsertError::IdentityChangeRequiresRename`) once the listing has
      any trades, income, or closing prices — a brand-new listing stays freely editable. The
      identity-continuity tests were updated to drive the rename through the action instead of a
      raw upsert, keeping their original assertions intact. 17 tests in `listing_rename.rs`, plus 2
      new `listing.rs` tests for the PUT restriction
- [x] `closing_price::Market` gained `symbol_override` (in-memory only, set by backfill's optional
      `symbol` param — recovers a pre-rename date range under the old symbol without touching the
      listing row); `yahoo_symbol` resolves `symbol_override` → `listing.price_symbol` → the derived
      mapping. The scheduled fetch deliberately stays on the listing's current symbol (Yahoo serves
      the full history under the current symbol in the common case). Tests cover the precedence
      order and the symbol reaching the fetcher end to end (a concrete `Arc<StubFetcher>` kept
      alongside the trait-object `SharedFetcher` so a test can inspect calls after the request)
- [x] `fetch_and_store` distinguishes a provider call returning **zero** candles across the whole
      requested window from a partial result with a data gap on one date — the former (the
      wrong/renamed/delisted-symbol case) now stores a message naming the symbol and pointing at the
      fix on every date, instead of the generic per-day message indistinguishable from a transient
      outage. `reports::health::HealthReport` gained `errored_prices` (`listing_id`, `ticker`,
      `errored_days`, `latest_errored_date`, `latest_error`, newest error first); the `#/prices`
      screen surfaces it with a Backfill action that pre-fills the existing backfill form
- [x] `domain::listing_identity::RenameHistory` (`ticker_as_at(listing_id, date, current_ticker)`,
      pre-loaded once per report like `infra::fx::FxRates`) resolves the ticker in effect at a given
      date across a listing's rename chain. Applied in the Annual Tax Report (a disposal group's
      heading names the ticker as at its most recent disposal in the year; an income row resolves at
      its own date) and the listing activity ledger (a rename appears as a `Ticker/exchange change`
      event, chronologically ordered, naming the exchange too when the rename moved it). A
      speculative `label_with_former` helper was dropped before landing — neither caller turned out
      to have a bare ticker string to decorate, so it would have been unused dead code. Every other
      report keeps showing the current ticker (the ATO view: a rename is the same security)
- [x] Confirmed `trades.settlement_date` is a stored column computed once at write time only when
      the request omits it (`entities::trade::http.rs`), never re-derived at read time — so an
      exchange change is safe for existing trades and only affects a trade re-saved afterward without
      an explicit `settlement_date`. Documented as a Known limitation rather than a code change
- [x] Docs: `docs/API.md`'s "Ticker or name changes" paragraph rewritten for the rename action (new
      endpoints, request/response shapes, the identity-preservation guarantee), the Closing Prices
      and Health sections updated for `price_symbol`/`symbol`/`errored_prices`, the Listing Activity
      and Annual Tax Report sections noting the as-at ticker resolution, two new Known-limitations
      entries (exchange-change recomputation; snapshot ticker labels are display-only), and the 201/
      422 response-code catalog entries; `docs/SCHEMA.md` (phase 1) and a README Features bullet.
      `src/doc_checks.rs` pins the new Known-limitations text and the documented endpoints/response
      shapes (`known_limitations_document_exchange_change_recomputation`,
      `known_limitations_document_snapshot_ticker_labels_are_display_only`,
      `listing_rename_action_documented`)
- [x] Verified: `cargo build`/`cargo test` (1192 passed, warning-free), `cargo fmt --check` and
      `cargo deny check advisories` clean, `node --test 'src/web/*.test.js'` (55 passed),
      `scripts/ui-smoke.sh` clean. End-to-end over HTTP against a scratch DB: created a listing,
      recorded a pre-rename Buy and dividend, renamed it, confirmed the trade/listing_id and the
      chain were correct, confirmed a bare `PUT` retrying the ticker change now 422s, recorded a
      post-rename Sell, confirmed the FY2024 tax report showed the dividend under the **old** ticker
      and the FY2025 report showed the disposal under the **new** one, confirmed `errored_prices`
      appeared correctly after a real (intentionally-failing) backfill with an explicit `symbol`
      override, and confirmed the listing activity ledger showed the rename in chronological order
      between the Buy and the Sell
