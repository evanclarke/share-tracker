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

## Price fetching and snapshot valuation disagree about renames, sales and splits (2026-07-28)
`reports::valuation::stored_valuations` refuses a date unless every listing held *as at that date*
has a final, ok stored close; price collection is what keeps that satisfiable. The two paths have
drifted apart four ways, leaving dates the snapshot job demands forever and collection can never
fill. The prompting case is the LAAC → LAR rename (`listing_renames` id 1), but only the first two
items are rename-specific. Evidence in the live DB: listing 8 (LAR) holds `ok` rows back to
2021-03-01 — Yahoo's `LAR` series, which maps onto the *pre-demerger old-LAC* prices, stored as if
they were LAAC's — while sibling listing 7 (LAC) has 187 errored days over 2023-01-03..2023-09-29
for the same period under a symbol that no longer serves it. Two listings, one window, opposite
wrong answers, neither reachable by the scheduled job.
- [x] Resolve the provider symbol **as at each date**: `yahoo_symbol` reads only the listing's current identity, and `fetch_and_store` issues one provider call for the whole `from..=to`, so a window straddling a rename cannot be right. Resolve from `listing_renames` (reusing `domain::listing_identity`, which already does this for the tax report and activity ledger) and split a straddling range into one provider call per identity segment
- [x] Resolve the **exchange calendar as at each date**: `load_market` builds holidays/timezone/close from the listing's live `exchange_mic`, so after a cross-exchange rename pre-rename trading days resolve against the new calendar — on both the fetch and the valuation path. Collection then requests days the old exchange was shut (permanent errored rows) and `stored_valuations` picks valuation days that were never fetchable
- [x] Align collection's held-set and window with the snapshot catch-up: `run_collection` uses `db_held_listing_ids(pool, None)` (held *now*) while `stored_valuations` uses `Some(date)` (held *then*), so a listing sold today stops being fetched while snapshot dates inside the catch-up window still demand its prices; and `COLLECTION_LOOKBACK_TRADING_DAYS = 7` (~9–11 calendar days) is narrower than `CATCHUP_LOOKBACK_DAYS = 14`, so days 11–14 back are retried forever and refilled never
- [x] Re-base allocations across splits in `db_held_listing_ids`: it subtracts `quantity_allocated` (sale-date units) raw from as-acquired Buy units, while `portfolio::db_holdings_on` and `unrealised_gains` re-base via `sold_in_acquired_units`. With a split between a Buy and a Sell they disagree, and `snapshot::generate` stores the snapshot anyway with `market_value = None` — silently unvalued, contradicting the module's own guarantee — or blocks a date on a security already fully sold
- Implementation notes (2026-07-28):
  - `domain::listing_identity` grew `Identity { from, ticker, exchange_mic }`, `identities()` (the
    listing's contiguous spans, the last always carrying the listing's *current* row) and
    `identity_as_at()`; `ticker_as_at` is now a thin delegate, so `tax_report`/`activity` were
    untouched. The module has three callers now, for two reasons — two presentational, one
    (price collection) a correctness one
  - `closing_price::Market` became an identity timeline (`Vec<MarketIdentity>`, private) with
    `identity_at` / `current` / `identity_segments`. `is_trading_day` and
    `latest_trading_day_on_or_before` resolve per date; `tz`/`close_time`/
    `latest_complete_trading_day` stay on the current identity, since "now" is by definition after
    any rename. `load_market` loads the chain and one exchange + holiday set per *distinct* MIC.
    New `exchange_holiday::db_holiday_dates_for(mic)` — the listing-joined
    `exchange_holidays_for_listing` can only answer for today's exchange, and settlement still
    uses it
  - `yahoo_symbol` takes a date; precedence is one-off `symbol_override` → `price_symbol` **only on
    the current span** → the derived mapping over the as-at ticker + MIC. `yahoo_symbol_now` is the
    live-quote form. `fetch_and_store` splits its dates by `identity_segments` and makes one
    provider call per span, with the zero-candle "symbol may be wrong/renamed/delisted" detection
    judged per span so the message names the symbol that actually came back empty
  - `COLLECTION_LOOKBACK_TRADING_DAYS: usize = 7` became `COLLECTION_LOOKBACK_DAYS: i64 = 14`
    (calendar days), and `snapshot::CATCHUP_LOOKBACK_DAYS` is now *defined as* that constant so the
    two windows cannot drift apart again. `run_collection` takes its candidates from the new
    `db_listing_ids_held_between(from, to)` instead of the live holdings
  - `db_held_listing_ids` now re-bases each allocation with
    `corporate_action::sold_in_acquired_units` over `db_share_split_events`, floored at nil per
    parcel — the same shape as `portfolio::db_holdings_on`. The old comment claiming splits
    "can't change whether the result is positive" was simply wrong, since nothing re-based
  - `test_support::QuoteStub` gained `with_symbol_closes(symbol, currency, closes)`: canned candles
    keyed by *provider symbol*, so a stub can model a provider that serves history only under the
    symbol in force at the time — the shape a rename produces
  - Tests: 4 new in `listing_identity.rs`, 10 in `closing_price.rs` (as-at symbol, as-at exchange
    suffix, `price_symbol` scoped to the current span, as-at holiday calendar, per-identity call
    splitting, self-healing pre-rename backfill with no `symbol` param, per-span dead-symbol
    message, a listing sold mid-window still collected, the window-covers-catch-up pin, and the
    held-set matching `portfolio::db_holdings_on` across both a 2:1 split and a 1:10
    consolidation), 2 in `snapshot.rs` (a pre-rename date valued end-to-end from what collection
    filled; a split holding never stored unvalued), and 1 new `doc_checks` pin — 1252 tests green
  - Docs: the `docs/API.md` Known limitation was **narrowed** to settlement only ("Settlement dates
    follow the listing's *current* exchange…") — the price half is fixed, and the entry now says so
    explicitly; its `doc_checks` pin was updated to match. The Closing prices and Listings sections
    describe as-at symbol resolution, per-identity calls, the `price_symbol` scoping and the shared
    14-day window; README's rename and closing-price feature lines follow
  - Verified end-to-end against a **copy** of the live DB (the live file was never written to):
    a pre-rename backfill of LAR over 2021-02 with **no** `symbol` param went to Yahoo as `LAAC`
    (the ticker in force then; Yahoo has retired it, so the rows errored naming `LAAC` — the
    resolution is the point, and it would have said `LAR` before); a post-rename backfill over
    2025-02 fetched 5 ok rows under `LAR`; and a backfill straddling a synthetic effective date
    split correctly — the days before it fetched real prices under the old ticker, the days from it
    errored under the new one, i.e. two provider calls, not one. `POST /jobs/price-import` then ran
    clean (`failed=0`) over the widened window

## Health check: held but never priced (REQUIREMENTS 2026-07-28)
`reports::health`'s `errored_prices` only catches a listing whose fetches *fail* — a row exists
with `status = 'error'`. The case that actually bit leaves no row at all: a day that was held and
never fetched, which is silent and permanent. Listing 7 (LAC) was bought 2021-03-25 but entered
five years later, so nothing ever attempted those days; the only symptom was 544 snapshots stuck
stale over exactly 2021-03-25..2022-09-19, and by the time it was found Yahoo no longer served
`LAC` before 2023-10-02, so the range was unrecoverable. It recurs whenever a trade is entered
later than the 14-day `COLLECTION_LOOKBACK_DAYS` window on a listing not otherwise held — an
established workflow here, since entry is batched from the statement archive.
- [x] `GET /reports/health` gains an `unpriced_days` list, the missing-row counterpart of `errored_prices`: for each date in a listing's held span, its **valuation day** (`Market::latest_trading_day_on_or_before`) has no stored row at all. Defined as exactly what `reports::valuation::stored_valuations` asks for, so there are no false positives; a day whose stored row is errored stays in `errored_prices` — the two lists partition the problem
- [x] Exclude days whose close is not final yet (`Market::latest_complete_trading_day`), so today and an unsettled crypto candle never appear; use the same held-as-at-that-date rule as the valuation path (`closing_price::db_held_listing_ids(pool, Some(date))`), so a fully-sold listing stops being reported after its sale and a sold-then-rebought listing is covered for both spans
- [x] Row shape mirrors `errored_prices` — `listing_id`, `ticker`, `unpriced_days`, `earliest_date`, `latest_date` — ordered by `earliest_date` so the oldest (least recoverable) hole reads first
- [x] Read each listing's stored dates once into a set and walk its held span in memory (one query per listing, no per-day round trip), following the existing `FxRates`/`RenameHistory` pre-loading pattern — a naive per-listing-per-day walk over six years of history is thousands of iterations
- [x] Surface on the `#/prices` screen beside the errored-price list, reusing its existing Backfill action; UI item asserted against the served bundle per the web-testing convention
- [x] Tests: a held day with no row is reported; an errored day is *not* (it belongs to `errored_prices`); a non-trading day and a not-yet-final close are not; a fully-sold listing isn't reported for dates after its sale; a hole straddling a rename resolves its trading calendar as at the date; a fully-priced database reports an empty list
- [x] Docs: `docs/API.md`'s Health section (the new list, its fields, and the `errored_prices`/`unpriced_days` partition), plus README's Features list if the health check is described there. No schema change and no migration — reads `trades`, `parcel_allocations`, and `closing_prices` only
- [ ] Deliberately NOT in scope: auto-backfilling what it finds. The check reports; closing the hole stays a deliberate act (`POST /closing_prices/backfill`, or a manual price for a day the provider can never serve) — a silently auto-filled hole is how the wrong series gets in
- Implementation notes (2026-07-29):
  - The held-span question needed a holdings model that answers *many* dates, so
    `closing_price::db_held_listing_ids`'s body became `HeldTimeline` (`closing_price.rs`): three
    queries (Buy/DRP parcels, allocations joined to their sale date, split events) loaded once, each
    allocation re-based to its parcel's as-acquired units at load time
    (`corporate_action::as_acquired_quantity`), then `held_listing_ids(as_of)` /
    `listing_ids()` / `held_spans(listing_id, until)` answered in memory.
    `db_held_listing_ids` is now a thin wrapper over it, so the "held" rule stays single-sourced
    and the existing callers (valuation, snapshot generation) are unchanged
  - `held_spans` evaluates the holding only at acquisition/sale dates and holds it constant in
    between — a listing's quantity cannot change on any other date — so the six-year walk is over
    calendar dates, not per-date holding sums. Spans are clipped at the caller's `until`
    (each market's `latest_complete_trading_day`) and merged when adjacent; a sold-then-rebought
    listing yields one span per holding period
  - `reports::health::db_unpriced_days` walks each held span, maps every calendar date to its
    valuation day and collects the **distinct** days with no stored row into a `BTreeSet` — so a
    weekend and the Friday it values at are one hole, not three, and `earliest_date`/`latest_date`
    fall out of the set's ends. Stored dates are read once per listing (any status: an errored day
    is `errored_prices`' to report). `db_health` gained a `now: DateTime<Utc>` parameter beside
    `today` for the not-final-yet cut-off; the handler passes `Utc::now()`
  - Deliberately *not* on the caller's read transaction (`load_market` is pool-based, and a hole is
    a hole whichever snapshot it is seen in) and deliberately *not* on the cross-view health banner:
    an unrecoverable hole (LAC 2021-03..2022-09 — Yahoo will never serve it) would nag on every
    screen forever with no way to clear it. The `#/prices` screen is where the fix lives, so that is
    where it is reported
  - Tests: 8 new in `reports::health::tests` (a held day with no row; an errored day excluded; a
    weekend + the ASX King's Birthday not holes; a close not final yet, before and after the ASX
    close on the same day; a fully-sold listing not reported past its sale; a hole straddling an
    ASX→NYSE rename walked on each date's own calendar; a fully-priced DB empty; oldest hole first)
    plus `web::tests::unpriced_days_ui_present` — 1261 green, clippy/fmt clean
  - Verified live: a fresh DB seeded with the demo fixture reports
    `{"ticker":"VAS","unpriced_days":646,"earliest_date":"2024-01-10","latest_date":"2026-07-29"}`
    in ~10 ms, and `scripts/ui-check.sh --seed demo '#/prices'` renders both rows (VAS before
    VDHG — oldest hole first) with the Backfill button pre-filling the form over exactly the hole

## PRODUCTION CLEANUP (completed 2026-08-21): clear LAC's 635 borrowed price rows on the deployed database

**Done 2026-08-21 — this section is the record of an operational runbook, not code that was written.** The procedure was run against the live database on 2026-08-21 and every figure it predicted came back exactly; the step-by-step outcome is in the *Development prerequisites* block below, under the host-upgrade item. The original framing follows, unchanged. It is recorded here so the work is
not lost, but nothing in it is done by editing this repository — it is a procedure to run against
Evan's deployed database once the prerequisites below are released. Evan asked (2026-08-20) to clear
the rows and to resume the job another day.

### What is wrong with the data

Listing 7 (`LAC`, held 2021-03-25 → 2023-10-03) carries **635** price rows dated before its
demerger, and every one is byte-identical to listing 8 (`LAR`/`LAAC`)'s row for the same date — 635
identical, 0 differing, 0 missing. They are another security's prices. Yahoo serves no `LAC` candle
before 2023-10-02 at all (HTTP 400 on every earlier day), so there was nothing else to reach for at
the time. See the `## LAC's whole pre-demerger price history is LAR's series` section above for the
full evidence; in brief:

| Rows | Dates | `origin` | How they got there |
| ---: | --- | --- | --- |
| 375 | 2021-03-25 → 2022-09-19 | `manual` | Hand-copied from listing 8, with `sourced_from`/`reason` stating plainly that the period is "unblocked, not accurate" |
| 260 | 2022-09-20 → 2023-10-02 | `fetched` | `POST /closing_prices/backfill`'s one-off `symbol` override, which recorded nothing about the symbol used |

Effect: **922 snapshot dates / 2,766 stored snapshot rows** value LAC at LAR's price. At 2023-09-29
the stored `portfolio_overview` row reads `market_value` A$11,123.21 against `total_cost_base`
A$19,869.26 — a 44% unrealised loss — where old Lithium Americas closed near US$16.85 (≈ A$27,400).
**No tax figure is affected**: closing prices feed valuation only, never cost base or proceeds.

### Development prerequisites (these *are* repo work, and are not all done)

- [x] `listings.unpriced_before` — a date before which the provider has no series, excluding the
  holding from valuation and flagging the snapshot partial. Decided 2026-08-20 and **in progress**;
  see the `## There is no "unpriced *before*" counterpart` section for the decision and its
  reasoning. Tick this only once that section is closed and archived.
  - Done 2026-08-20 (migration 0037); the section is closed and archived in `DONE/reviews.md`. The
    snapshot carries `holding_excluded` plus an `excluded_holdings` list naming the absent holding
    and why, and setting it on listing 7 was rehearsed against an upgraded copy of the deployed
    database: 2023-09-29's total drops from A$495,429.52 to A$484,306.31, LAC's row unvalued with the
    reason. Note that the marker **supersedes** the stored rows for the span, so step 4 of the
    procedure is no longer what makes the totals honest — it is now housekeeping on rows nothing
    reads.
- [x] **An `ok` row must become deletable inside an `unpriced_before` span.** Today
  `DELETE /closing_prices/:listing_id/:price_date` refuses every `ok` row, so all 635 are
  unremovable through the API and this cleanup cannot be performed at all. The relaxation is
  principled and narrow rather than a general loosening: once a listing declares `unpriced_before`,
  dates in that span are *by declaration* not read by valuation, so deleting a stored price there
  cannot punch a hole in a valued series — which is the only reason the rule exists. A bulk form is
  probably wanted too; 635 single-date DELETEs is not a runbook.
  - Done 2026-08-21; see the "The rows cannot be cleared" item in the section above for the
    reasoning and the tests. The refusal is relaxed inside an `unpriced_before` span only (and
    deliberately **not** inside an `unpriced_from` run, where the last stored close *is* read),
    and `POST /closing_prices/clear_unpriced_before` is the bulk form — body `{ "listing_id": 7 }`,
    no date range, one transaction, idempotent, reporting the row count. The whole procedure below
    was rehearsed on an upgraded copy of the 2026-08-16 backup and its numbers are this section's.
- [x] The deployed host must be upgraded first. It was at **migration 21**; this repo is past 36. The
  upgrade was rehearsed clean against a copy (21 → 36, every row count preserved,
  `integrity_check` and `foreign_key_check` clean, all seven annual tax reports and every
  cross-check report 200) — but none of it had been released.
  - Released 2026-08-21 as **v0.12.0** (`54f4012`), 137 commits and 18 migrations on from v0.11.0.
    Evan upgraded the host and took the pre-upgrade backup
    (`share-tracker-2026-08-21-102249-pre-0.12.0.db`, at migration 21 — the rollback point). The live
    database came up at **migration 39**, and `GET /reports/health` immediately answered with the new
    `duplicate_price_series` check firing exactly as predicted: LAC/LAR, 634 identical days,
    375 manual + 259 fetched.

**The procedure below was run against the live database on 2026-08-21 and is complete.** Every figure
it predicted came back to the last decimal place. What was done, in the order the runbook gives:

1. Pre-cleanup backup at migration 39 taken with `POST /jobs/backup?suffix=pre-lac-cleanup` →
   `share-tracker-2026-08-21-102806-pre-lac-cleanup.db` (verified by the job: `integrity_check` plus
   a migration match). The migration-21 pre-upgrade backup is kept alongside it.
2. The whole sequence rehearsed first against a **copy of that backup**, served locally on a throwaway
   port — not against the 2026-08-16 backup the earlier rehearsal used, so the rehearsal input was
   byte-for-byte the live data.
3. `PUT /listings/7` with `unpriced_before: 2023-10-02` → 204.
4. `POST /closing_prices/clear_unpriced_before` `{"listing_id": 7}` → **634 deleted** (375 manual +
   259 fetched), the 635th being 2023-10-02 itself, which is outside the span and is LAC's own row.
   `row_history` went 28 → 663: all 634 deletions audited, and all **375** hand-entered rows' "this
   period is unblocked, not accurate" note preserved in the trail.
5. `POST /report_snapshots/regenerate_all` `{"to": "2023-10-02"}` → **1,128 regenerated, 0 blocked**,
   leaving only the 3 rows that were already stale in the backup.
6. Spot-check 2023-09-29: the stored `portfolio_overview` total fell from A$495,429.52 to
   **A$484,306.31** — smaller by exactly **A$11,123.21**, the figure that was another company's price
   — with LAC's row unvalued and `excluded_holdings` naming it and why. 2023-10-02 values LAC at its
   own A$16,744.99.

Health afterwards: `duplicate_price_series` **empty**, `errored_prices` empty, `failed_jobs` empty,
and `demergers_missing_close` down to `adjusted_days: 1, manual_days: 0` — the single genuine
2023-10-02 row, which is what the stated-close item above was waiting for.

### The procedure, once the above are released

1. Take a fresh backup (`POST /jobs/backup?suffix=pre-lac-cleanup`) — the deployed host's own job.
2. Rehearse the whole sequence against a **copy** of that backup before touching the live database.
3. `PUT /listings/7` with `unpriced_before: 2023-10-02`.
4. Clear the superseded rows in one request: `POST /closing_prices/clear_unpriced_before` with
   `{ "listing_id": 7 }`. It clears exactly the span the marker set in step 3 supersedes and answers
   `{ "listing_id", "unpriced_before", "deleted" }`; on the rehearsal `deleted` was **634**, and a
   second call reported 0. Note the count: 634, not 635 — the 635th is 2023-10-02 itself, which is
   *not* in the span (`price_date < unpriced_before`) and is the one row that is genuinely LAC's own
   (10.13, against listing 8's 6.453301 for the same day). That is the row a stated close would
   re-base, and it is meant to stay.
5. Regenerate the affected snapshots (`POST /report_snapshots/regenerate_all` with
   `{ "to": "2023-10-02" }`) and confirm the totals come back flagged partial rather than blocked.
   Leave `from` out rather than passing 2021-03-25: setting the marker stales the whole **prefix**
   before its date, which reaches back past LAC's own first holding to the first-ever-held date
   (2020-08-31 on the rehearsal — 206 dates a 2021-03-25 start would have left stale). The rehearsal
   regenerated 1,128 dates, 0 blocked, leaving only the 3 rows already stale in the backup.
6. Spot-check 2023-09-29: the LAC row should be absent and the total lower by A$11,123.21, with the
   snapshot naming LAC as excluded. On the rehearsal the stored `portfolio_overview` total went from
   A$495,429.52 to A$484,306.31, LAC's row carrying `price_unavailable` and the snapshot
   `holding_excluded` with `excluded_holdings` naming it; 2023-10-02 valued LAC at its own
   A$16,744.99. `GET /reports/health`'s `demergers_missing_close` then reads `adjusted_days: 1,
   manual_days: 0` — the single genuine row, which is what the stated-close item above is waiting
   for.

### Things to know before running it

- **Nothing is destroyed.** `closing_prices` is an [audited table](docs/SCHEMA.md), so every deleted
  row lands in `row_history` with its figure and its `sourced_from`/`reason` intact. The 375 manual
  rows' careful note about what they were survives; it just stops being read as a valuation.
- **The trade Evan accepted**, worth re-reading before running: 922 dates currently report a
  wrong-but-present total, and afterwards report a total that omits a holding he really owned. The
  true figures are not obtainable — old Lithium Americas' pre-separation closes are not served by the
  provider under any symbol. Neither state is right; the second is honest about it.
- **Do not record a demerger stated close on action 1 until this is done.** Its factor derives from
  the last pre-demerger stored row (2023-10-02, `10.13`), which is LAC's own demerger-adjusted figure,
  while the 635 behind it are LAR's — one factor cannot serve both, and stating one first would scale
  the wrong series by a plausible-looking number.
- **Where the data is.** The deployed database is on `bigbrain.lan:3000`. The copy in this repo
  (`share-tracker.db`) is **stale** — last written 2026-07-30, still at migration 21 — and is not the
  live data. The backup Evan fetched on 2026-08-20 is `share-tracker-2026-08-16-000000.db`; an
  upgraded scratch copy used for the rehearsals was at
  `scratchpad/upgrade-rehearsal.db` (session-local, will not survive).
