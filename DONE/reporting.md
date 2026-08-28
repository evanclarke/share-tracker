# Done — Reporting — Portfolio, Gains, Tax, Snapshots & Performance

## Reporting — Portfolio Overview
- [x] Current holdings: aggregate open parcels by listing (quantity, average cost base)
- [x] Accept current market prices as input to materialise portfolio value
- [x] API endpoint for portfolio overview
- [x] Tests: holdings aggregation after buys and sells

## Reporting — Unrealised Gains/Losses
- [x] Calculate unrealised gain/loss per holding (market value vs cost base)
- [x] Apply 50% CGT discount indicator for parcels held > 12 months
- [x] API endpoint for unrealised gains/losses
- [x] Tests: gain/loss calculation, discount eligibility

## Reporting — Realised Gains/Losses
- [x] Calculate capital gain/loss per sale using allocated parcels and adjusted cost bases
- [x] Apply CGT discount (50%) for parcels held > 12 months
- [x] API endpoint for realised gains/losses
- [x] Tests: specific parcel sale, CGT discount eligibility

## Reporting — Tax
- [x] Aggregate all assessable income components by tax year
- [x] Aggregate franking credits, foreign tax offsets, TFN withholding tax by tax year
- [x] Include AMMA attributed income components in tax year totals
- [x] Include LIC capital gain deductions
- [x] Exclude conduit foreign income from assessable totals
- [x] API endpoint for tax summary by year
- [x] Tests: full-year tax summary with mixed income types

## Open-parcel cost-base inventory report
(REQUIREMENTS "Planned Enhancements — Open-parcel cost-base inventory report". Portfolio overview only aggregates per listing.)
- [x] New report listing every open (unsold) parcel: listing, acquisition date, original cost base, cumulative AMIT reductions to date, remaining quantity, remaining adjusted cost base (AUD) — `GET /portfolio/open-parcels` (`src/reports/open_parcels.rs`): one row per Buy/DRP trade not fully consumed by parcel allocations, with `trade_id`, `listing_id`, `ticker`, `acquisition_date`, `original_quantity`, `remaining_quantity`, `original_cost_base`, `amit_cost_base_reduction` (full cumulative reduction, even past the E10 floor), `remaining_cost_base` (`max(original − AMIT, 0)` pro-rated to remaining units); all money AUD at the parcel's buy-month rate via `infra::fx::to_aud`; sorted by listing, acquisition date, trade id
- [x] Web UI view for the open-parcel inventory report (routed through the shared filterable table) — `REPORTS` entry `open-parcels` in `app.js`; GET reports render through `dataTable` → `filterableTable` (`web::tests::open_parcels_report_ui_present`)
- [x] Tests: open parcels listed with correct remaining quantity and adjusted cost base after partial sells and AMIT adjustments — `open_parcels::tests`: `db_open_parcel_listed_with_original_figures`, `db_partial_sell_pro_rates_remaining_cost_base`, `db_fully_sold_parcel_excluded`, `db_amit_reduction_reported_and_netted_off`, `db_e10_floors_remaining_cost_base_at_nil`, `db_non_aud_parcel_converted_to_aud`, `db_sorted_by_listing_then_acquisition_date`, `api_get_open_parcels`
- [x] README sync: new report endpoint + web-frontend mention — "Open parcels" subsection under Portfolio reports; report list in the Web frontend section

## Tax-return export
(REQUIREMENTS "Planned Enhancements — Tax-return export". Reports are JSON/HTML only.)
- [x] Export the tax summary and net-capital-gain reports to a downloadable, tax-return-ready format (CSV at minimum) — `GET /portfolio/tax-summary/export` and `GET /portfolio/net-capital-gain/export` (registered in each report's router) serve the same per-year rows as CSV via the shared `reports::export::csv_response` helper (`text/csv; charset=utf-8` + `Content-Disposition: attachment; filename="<report>.csv"`): an explicit header record naming the report's fields in declaration order, then one record per financial year (Decimal fields keep their precision — rust_decimal serializes as plain decimal strings). The csv writer is not `flexible`, so a header/struct drift fails the request loudly instead of shipping misaligned columns; an empty report still exports the header row
- [x] Web UI export action on those report views — `export: true` on the two `REPORTS` entries; `viewReport` renders an "Export CSV" link to `<api>/export` (the Content-Disposition makes the browser download it), styled as a button (`a.export-link`)
- [x] Tests: the export endpoint returns the report rows in the chosen format with the expected columns — `tax_summary::tests::api_export_returns_csv_with_expected_columns` / `api_export_of_empty_report_still_returns_header` and the same pair in `net_capital_gain::tests` (status, content-type, attachment filename, header line == the report's column list, per-year record figures incl. decimal precision); `reports::export::tests` (header+rows rendering, empty-report header row, header/struct drift is an error); `web::tests::report_export_ui_present` (export flag, `/export` link path, and the action label ship in the bundle)
- [x] README sync: export endpoints + response content types — export endpoint blocks under the Tax summary and Net capital gain report sections, a Tax-return CSV export Features bullet, the Export CSV action in the Web frontend section, and the `200 OK` Response codes row notes the `text/csv` content type

## Performance / return metrics
(REQUIREMENTS "Planned Enhancements — Performance / return metrics".)
- [x] NEEDS CLARIFICATION: decide whether to report investment performance (total return, money-weighted return/IRR, income/dividend yield per holding and overall) — RESOLVED 2026-06-07: **in scope** — total return (absolute AUD + percentage), annualised money-weighted return (IRR over the dated external cash flows), and income yield (trailing 12 months' income over market value), per holding and overall
- [x] If in scope: implement the chosen performance report(s) + Web UI view — `POST /portfolio/performance` (`src/reports/performance.rs`): one row per holding (listing × holding account) plus a final OVERALL row, valued at `as_of_date` (default today; later flows ignored) with supplied AUD prices. Cash-flow based: out = each Buy/DRP parcel's AUD cost on its trade date (deemed-acquisition-month conversion for rollover parcels); in = each Sell's AUD net proceeds, each distribution's cash (franked+unfranked+foreign−foreign tax−TFN, the DRP reinvestable-cash definition — franking credits aren't cash), and the holding's market value at `as_of_date`. Internal movements (transfer / scrip-for-scrip / demerger groups, identified by their provenance columns) are valued at the carried cost within each holding — the source exits without gain, the destination carries the cost base — and skipped entirely in the OVERALL row, so portfolio figures are unaffected by moving parcels around; AMMA statements are attribution, not cash, and are excluded. Metrics: `total_return` (AUD) + `total_return_pct`, `money_weighted_return_pct` (annualised IRR over the dated flows, actual/365, Decimal bisection via `rust_decimal`'s `maths` feature — never `f64`), `income_yield_pct` (trailing 12 months' income / market value). An open holding with no supplied price reports null market-dependent metrics (never a silently wrong figure); the OVERALL row only when every open holding is priced. SPA `REPORTS` entry `performance` (price + as-of-date form, shared with unrealised gains). README: Features bullet + a Performance subsection under Portfolio reports + the web-frontend view list
- [x] Tests: performance metrics computed correctly over a known trade/income history — `performance::tests`: `db_open_holding_reports_value_return_and_yield` (invested/market value/total return abs+pct/trailing yield + the OVERALL row), `db_money_weighted_return_of_a_one_year_gain_is_exact` (1,000→1,100 over exactly 365 days = 10.0000% p.a.), `db_closed_holding_reports_realised_performance_without_prices`, `db_open_holding_without_price_has_unknown_market_metrics`, `db_trailing_yield_counts_only_the_last_years_income`, `db_transfer_is_internal_to_holdings_and_invisible_overall` (source exits at carried cost, destination shows the gain, OVERALL unchanged), `db_non_aud_invested_converts_to_aud`, `db_flows_after_as_of_are_excluded`, `db_empty_returns_empty`, plus `api_performance_with_prices_and_as_of_date`, `api_performance_without_body_defaults_to_today`; `web::tests::performance_report_ui_present`

## Daily closing prices and scheduled report snapshots
(REQUIREMENTS "New Requirements — Daily closing prices and scheduled report snapshots", added 2026-06-07. Closing prices for every held listing are collected after each exchange's trading day closes and stored as history; once the day's last close is in, the price-dependent reports run against the stored prices and persist a daily snapshot series — viewable and graphable, and invalidated by back-dated facts. The requirement flags two implementation-time decisions: the concrete price provider(s) and the crypto daily cut-off convention.)
- [x] NEEDS CLARIFICATION: choose the concrete price provider(s) behind the pluggable fetcher trait — coverage for the held exchanges (XNYS, XASX, …) and for crypto, free vs keyed access, key handling, rate limits — and document the choice. The requirement fixes only the trait, not the provider — RESOLVED 2026-06-07: **Yahoo Finance (unofficial chart API)** as the single initial provider: free and keyless, one provider covers all three asset classes (NYSE tickers plain, ASX via the `.AX` suffix, crypto as `BTC-AUD`/`ETH-AUD`), and its daily-OHLC history endpoint returns the full range in one call so backfill is ~one request per listing. The endpoint is unofficial (no ToS-blessed programmatic use; cookie/crumb requirements have come and gone) — that breakage risk is accepted and mitigated by the pluggable fetcher trait, which is the swap point if it breaks (fallback candidates evaluated: Stooq + CoinGecko, keyed free tiers like Alpha Vantage ~25 req/day, paid EODHD — all rejected as either needing coverage verification, too quota-tight, or overkill for a handful of holdings). The implementation item must verify the endpoint's current shape live before building and document the request format + any UA/cookie handling alongside the fetcher — *implementation note 2026-06-07:* verified live; a bare curl now 429s without the cookie/crumb dance, so the fetcher uses the maintained **yfinance-rs** crate (request format + crumb handling delegated; documented in `closing_price`'s module docs). Its build.rs needs `protoc` (brew locally, apt step in ci.yml) and it pins reqwest 0.12 beside our 0.13 — accepted
- [x] NEEDS CLARIFICATION: fix the daily cut-off convention for exchange-less (Crypto) listings (e.g. UTC midnight) — they trade continuously, so "close of trading day" must be defined. This supersedes the "no crypto price feed" limitation for *stored history* only; report requests with explicitly supplied prices keep working unchanged — RESOLVED 2026-06-07: **UTC midnight**. It is the daily-candle boundary every provider's historical series keys on, so live collection and backfill produce identical figures and a re-fetch reproduces the stored row (the reproducibility property the errored-row re-run and backfill items depend on). Sydney midnight was rejected because no provider serves Sydney-keyed daily candles — live spot-at-midnight and UTC-derived backfill would disagree; collect-at-last-equity-close was rejected as a non-standard, unbackfillable data point that shifts with US DST. Consequence: a stored crypto price for date D is the candle closing 00:00 UTC end of D (~10–11 am Sydney the next morning); document this on the price-history table/endpoint
- [x] DB schema + migration: closing-price history table — listing FK, price date, price (TEXT Decimal, in the listing's quote currency), provenance (source, fetched-at timestamp), CHECK-constrained status enum (ok / error, with error detail); one row per (listing, date) enforced by UNIQUE; no data dropped — migration `0018_closing_prices.sql`: `closing_prices` keyed PK (listing_id, price_date), CHECKs tying `price`/`error` nullability to `status`; also adds `exchanges.close_time` (TEXT `HH:MM` local, default '16:00') so "after each exchange's close" is per-exchange data, not a hardcoded hour (wired through the Exchange entity + UI). Tests: `closing_price::tests::db_check_constraints_tie_price_and_error_to_status`
- [x] Price-fetcher trait + the chosen provider implementation(s): fetch a listing's closing price for a date; pluggable so providers can be swapped; a failure is an error result — never a silent zero or skipped row — `closing_price::PriceFetcher` (per-listing daily closes over a date range; each provider does its own symbol mapping + candle-timestamp→trading-date conversion) + `YahooFetcher` via the **yfinance-rs crate** (chosen over hand-rolling: it maintains the cookie/crumb workarounds a bare curl trips over (429) and the request format, returns Decimal prices with the quote currency attached; cost: its build.rs needs `protoc` — installed locally via brew, added to ci.yml — and a second reqwest 0.12 in the tree). Symbols: XASX→`.AX`, XNYS/XNAS→plain, crypto→`TICKER-<currency>`; unmapped exchange → error row. Provider currency cross-checked against the listing (mismatch = errored row, never stored as the wrong currency); float32 noise rounded to 7 significant digits (`clean_price`). Verified live 2026-06-07 against BHP.AX / ICE / BTC-AUD end-to-end (incl. the running server + `POST /jobs/price-import`). Tests: `yahoo_symbols_cover_asx_us_and_crypto`, `clean_price_strips_float_noise_and_keeps_tiny_prices`, `collection_records_currency_mismatch_as_error`
- [x] Scheduled collection via `infra/scheduler.rs` + `schedule.cron` (registry entry + cron line; runs through `run_job` so start/finish INFO logging holds): after each exchange's close, fetch that day's closing price for every listing with a non-zero holding; trading days only — skip weekends and the exchange's seeded holidays (`exchange_holidays`), a non-trading day stores no row and is not an error; Crypto listings collected once daily at the cut-off convention above — `price-import` job (`closing_price::run_collection`): held listings = Buy/DRP quantity minus parcel allocations > 0 (Decimal in Rust, no float SQL); per listing the *latest complete trading day* = today in the exchange tz if past `close_time` else yesterday, walked back over weekends/holidays (crypto: yesterday UTC, every day trades); days already stored ok are skipped so runs are idempotent. Two cron lines (17:30 weekdays for the ASX close; 11:30 daily for the prior NYSE close + crypto UTC cut-off — same job, idempotent). Tests: `collection_stores_price_per_held_listing_and_skips_non_held`, `collection_skips_a_day_already_stored_ok`, `db_close_time_gates_same_day_collection`, `db_weekends_and_holidays_walk_back_to_a_trading_day`, `db_crypto_cutoff_is_utc_midnight_with_no_holiday_calendar`, `collection_crypto_collected_daily_at_utc_cutoff`, `db_held_listings_excludes_fully_sold`, plus `scheduler::tests::embedded_schedule_is_valid` covering the new lines
- [x] A failed fetch is recorded as an errored row for that (listing, date) — never silently missing — and can be re-run on demand for just that day/listing (manual trigger endpoint); the re-run replaces the errored row — a provider failure, a missing candle on an expected trading day, and a currency mismatch each upsert `status='error'` + detail (and fail the job so the Jobs UI shows it, without stopping other listings); `POST /closing_prices/fetch {listing_id, price_date}` re-fetches one day and returns the freshly stored row (404 unknown listing; 422 close-not-final / non-trading day). Tests: `collection_failure_stores_errored_row_and_fails_the_job`, `collection_replaces_an_errored_row_once_the_provider_recovers`, `api_fetch_replaces_errored_row_and_returns_it`, `api_fetch_rejects_incomplete_and_non_trading_days`
- [x] Backfill on demand: endpoint taking a listing + date range (e.g. after importing an old trade: trade date to today); fetches trading days only and skips dates already stored ok — `POST /closing_prices/backfill {listing_id, from, to}`: `to` clamped to the latest complete trading day, missing trading days fetched in one provider call (Yahoo serves the whole range at once), expected-but-absent candles stored as errored rows; returns `{trading_days, already_stored, fetched_ok, errored}` (404 unknown listing, 422 from>to / no complete day in range). Tests: `api_backfill_fetches_only_missing_trading_days`, `api_backfill_records_missing_candles_as_errors`, `api_backfill_unknown_listing_404_and_bad_range_422`
- [x] Price-history endpoint: list stored prices filterable by listing and date range, including errored rows — `GET /closing_prices?listing_id=&from=&to=` (newest first). Test: `api_list_filters_by_listing_and_date_range_including_errors`
- [x] Web UI: price-history screen through the shared `filterableTable`, with the re-run and backfill actions — `viewClosingPrices` (`#/prices`, Reference data nav): listing-labelled rows with status badges, a per-row Re-fetch action driving `POST /closing_prices/fetch`, and a backfill form driving `POST /closing_prices/backfill`; the exchanges form gains `close_time`; the Jobs view describes `price-import`. Verified rendered in headless Chrome 2026-06-07. Test: `web::tests::closing_prices_ui_present` (+ `exchange_management_ui_present` asserting `close_time`)
- [x] Report-snapshot schema + migration: store each day's results of the price-dependent reports (portfolio overview, unrealised gains, performance) keyed by (report, date), with generated-at and a stale flag; result money fields TEXT Decimal, no data dropped — migration `0019_report_snapshots.sql`: `report_snapshots` PK (report, snapshot_date), `report` a CHECK-constrained enum (`portfolio_overview`/`unrealised_gains`/`performance`, typed `snapshot::ReportKind` in Rust), `generated_at`, `stale` CHECK (0,1), and the report's response rows as `rows_json` TEXT — money values inside are the API's Decimal strings inside a TEXT column, never a REAL. New table only, nothing dropped
- [x] Scheduled snapshot job (registry + `schedule.cron`): after the last relevant exchange close of the day, run the price-dependent reports using that day's stored closing prices — converted to AUD per the existing FX rules, since stored prices are quote-currency — and persist the results as that date's snapshot. A day whose price fetches failed has no trustworthy snapshot and shows as such (missing, distinguishable from stale) until the price re-run succeeds — `report-snapshot` job (`snapshot::run_snapshot_job`, daily 12:00 after the 11:30 price import): `latest_snapshot_date` finds the latest calendar date every held listing's valuation day (nearest trading day ≤ it) has a final close for, `generate` values each listing at that day's stored ok price `to_aud`-converted (no manual override — a missing RBA rate fails loudly) and stores all three reports in one transaction; any missing/errored price aborts the whole day with the blockers listed (nothing stored = missing ≠ stale) and fails the job so the Jobs UI shows it; a date already stored fresh is skipped (idempotent), a stale one is regenerated. To make a past day's snapshot the *actual* position then, `db_holdings` gained an `as_of` param and `db_unrealised_gains` now filters trades/sales/ROC/AMIT (by statement year end) to ≤ `as_of_date` — live endpoints unchanged (`None`/today)
- [x] On-demand snapshot generation for past dates whose prices have been backfilled — `POST /report_snapshots/generate {date?}` (date omitted = the latest fully-valuable day) shares `generate` with the job: held-as-at-the-date listings (dated `db_held_listing_ids`), each valued at its nearest trading day ≤ the date; 422 with the blocker detail (price missing → "backfill it", errored → "re-fetch it", close not final, nothing held)
- [x] Staleness invalidation, enforced at write time in the same transaction as the fact: adding, changing, or deleting any back-dated fact (trade, Sell, income, AMMA statement, corporate action + its operations, transfer, AMIT adjustment, …) marks every stored snapshot dated on or after that fact stale; stale snapshots are visibly flagged wherever shown and regenerable on demand — regeneration re-runs the report with the stored prices and the new facts, replacing the stale result — enforced by **database triggers** in 0019 (insert/update/delete on `trades`, `parcel_allocations` (dated via its sale trade), `income` (`date_paid`), `amma_statements`/`amit_adjustments` (statement `tax_year_end_date`), `corporate_actions`; an update invalidates from min(old, new) date), so every write path — entity CRUD, Sells, transfers, the corporate-action operations, DRP reinvestment, and any future code — invalidates atomically with the fact, none can bypass it; revising a stored ok closing price (or erroring it out) also stales from its date. Stale rows are flagged in the list/series endpoints and badged + hollow-pointed in the UI; regeneration = the same `POST /report_snapshots/generate`, which clears the flag
- [x] Snapshot viewing: endpoint(s) + web UI views through the shared `filterableTable`, plus a time-series graph (e.g. market value and unrealised gain over time) rendered without introducing a build step — `GET /report_snapshots` (metadata, filterable by report/date range), `GET /report_snapshots/{report}/{date}` (the stored rows), `GET /report_snapshots/series` (per-date portfolio AUD totals for graphing); SPA `viewSnapshots` (`#/r/snapshots`, Reports nav): generate/regenerate form + `filterableTable` list with stale badges and per-row View/Regenerate, `viewSnapshotDetail` renders a day's stored rows with a stale warning, and `seriesChart` draws market value + unrealised gain over time as hand-built inline SVG (polylines + gridlines, stale points hollow, `<title>` tooltips) — no chart library, no build step
- [x] Tests: collection stores a price per held listing per trading day and skips weekends/holidays/non-held listings; crypto collected at the cut-off with no holiday calendar; a failed fetch persists an errored row and the on-demand re-run replaces it; backfill fetches only missing trading days in the range; the snapshot job persists AUD report results keyed by date; a back-dated fact flags on/after snapshots stale (and leaves earlier ones alone) and regeneration with the new facts clears the flag; a failed-price day yields no snapshot (missing ≠ stale); price-history/snapshot views + the graph ship in the bundle (no-browser-harness convention) — *price half 2026-06-07:* every price-collection test listed against the checked items above (stub `PriceFetcher`). *Snapshot half:* `snapshot::tests::db_snapshot_job_persists_aud_report_results_keyed_by_date` (incl. the USD→AUD conversion and the fresh-skip), `db_weekend_snapshot_walks_back_to_each_markets_trading_day` (mixed ASX + crypto Saturday), `db_back_dated_fact_stales_on_or_after_snapshots_and_regeneration_clears` (trade/income/corporate-action insert + delete paths through the triggers; earlier snapshot untouched; regenerated rows carry the new parcel), `db_failed_price_day_yields_no_snapshot_until_the_price_rerun_succeeds`, `db_job_skips_when_nothing_held_and_generate_rejects_unfinal_dates`, `api_generate_list_get_and_series`, `api_generate_blocked_day_returns_422_with_detail`; as-of regressions `unrealised_gains::tests::db_facts_dated_after_as_of_are_excluded` + `portfolio::tests::db_holdings_as_of_a_past_date_excludes_later_facts`; UI `web::tests::report_snapshots_ui_present` (views, endpoints, SVG chart pieces, stale badge, job description, chart CSS); the new cron line is covered by `scheduler::tests::embedded_schedule_is_valid`
- [x] Docs sync: `docs/SCHEMA.md` (new tables + Relationships), `docs/API.md` (price-history / re-run / backfill / snapshot / regeneration endpoints + Response codes, and supersede the "market prices remain request-supplied" notes where stored prices now also serve), README Features bullet + web-frontend view list, `schedule.cron` comment lines for the new jobs, and the two Known limitations: intraday prices are not stored (one closing/reference price per listing per day), and a back-dated fact does not auto-backfill prices — backfill is on demand; it only flags snapshots stale — *price half 2026-06-07* (as noted above); *snapshot half:* SCHEMA `report_snapshots` block + the staleness-trigger paragraph and Relationships line; API Report snapshots section (endpoints, semantics, missing ≠ stale), Jobs registered-list update, 422 row extension, the Crypto "request-supplied" note superseded by stored history + the snapshot cross-reference under Overview, the unrealised-gains as-at-`as_of_date` semantics, the intraday/no-auto-backfill limitation extended with "only flags snapshots stale"; README Daily-report-snapshots feature bullet, web-UI bullet, scheduled-maintenance prose + cron sample; `schedule.cron` report-snapshot comment block; CLAUDE.md reports-folder note (snapshot.rs writes; staleness lives in 0019 triggers — new dated fact tables need matching triggers)

## Live current prices from the price source, with an as-of time
(REQUIREMENTS "Live current prices from the price source, with an as-of time", added 2026-06-08. Now that there's a real price source (Yahoo, via the `PriceFetcher` trait), the price-dependent reports/screens should fetch *current* prices from the source instead of returning empty valuations when the caller supplies no `prices` map. Each fetched price carries the provider's quote timestamp (as-of time) so the user sees how fresh the valuation is; explicitly supplied prices still override; a per-listing fetch failure degrades gracefully rather than zeroing. Stored closing-price history and daily snapshots are unchanged — this is on-demand live valuation.)
- [x] A "latest quote" fetch capability on the price source: extend the `PriceFetcher` trait (and `YahooFetcher`) to return the most recent available price per listing **with its provider quote timestamp** (the as-of moment), in the listing's quote currency — for exchange-listed and exchange-less (Crypto) listings alike. Stub fetcher for tests — added `PriceFetcher::latest_quote(&Market) -> LatestQuote { price, currency, as_of: DateTime<Utc> }` (`closing_price.rs`); `YahooFetcher` implements it via `yfinance_rs::quotes` (paft `Quote.price`/`as_of`), reusing `yahoo_symbol` so crypto (`BTC-AUD`) works alongside ASX/NYSE; prices `clean_price`d like closes. Two stubs: the daily-close `StubFetcher` gained `with_quote`/`latest_quote`, and a reusable `test_support::QuoteStub` (pub under `#[cfg(test)]`) the report tests layer as a `SharedFetcher`
- [x] Price-dependent reports (portfolio/valuation, unrealised gains, performance) fetch live prices by default: when the request supplies no explicit `prices` map, value each held listing from the latest fetched quote instead of returning empty `current_price`/`market_value`. Convert each quote to AUD via the existing FX rules before valuing — never mix currencies — shared `closing_price::fetch_live_aud_prices` (latest quote → currency cross-check → `infra::fx::to_aud` at the quote month, no manual override) and `resolve_live_prices` (skips overridden listings, honours the flag). The three handlers gained a `live` request flag and an `Option<Extension<SharedFetcher>>`; **live is off by default** so the deterministic ATO acceptance tests (which `POST {}` against the full `app::router`/`YahooFetcher`) never hit the network, while the web UI sends `live: true`. An explicit price is never fetched (it always wins)
- [x] Each valued holding carries its price's as-of time through to the report response (per-row), so the web UI and API consumers can see it; report exposes the set/range of as-of times for a summary "as at …" line — every row struct gained `price_as_of: Option<String>` (RFC 3339, `#[serde(default, skip_serializing_if=None)]` so old stored snapshots still deserialize and snapshot rows omit it) carrying the provider quote timestamp; the per-row set is the report's exposed range, which the UI rolls into one "as at …" line (`asAtSummary`, min–max of the row times)
- [x] Explicit supplied prices still override the fetched ones — the existing `prices` request body keeps working unchanged (what-if valuations; deterministic ATO acceptance tests) — a listing in `prices` is excluded from the live fetch and applied verbatim (no `price_as_of`); `prices`-only requests behave exactly as before
- [x] A failed/unavailable live fetch for a listing is surfaced, not silently zeroed: that holding shows no current value with a reason while the rest of the report still values (consistent with the never-silent-zero rule) — `fetch_live_aud_prices` returns `Result<LiveValuation, String>` per listing; an `Err` (provider failure, currency mismatch, or no AUD rate for the quote month) sets the row's new `price_unavailable: Option<String>` and leaves `current_price`/`market_value` null, the rest of the report unaffected
- [x] Web UI surfaces the as-of time near the valuation (per-row and/or a summary "as at …" line) on the price-dependent screens — the shared POST-report runner (`viewReport`) now sends `live: true`, runs on first load (no manual price entry needed), relabels the price form as overrides, and prepends a "Live prices as at …" line (per-row `price_as_of` rolled up, plus a count of holdings with no live price); `price_as_of` renders local-with-UTC-tooltip via the existing timestamp cell renderer
- [x] Docs sync: `docs/API.md` documents the new default (live-fetched), the as-of time field, and the override; README Features notes live valuation — `docs/API.md` gained a **Live valuation** subsection (as-of time, override, graceful failure, off-by-default) linked from all three report sections, with the `live` flag and `price_as_of`/`price_unavailable` fields documented; README adds a **Live valuation** feature bullet
- [x] Tests: live fetch fills valuations when no prices supplied; the as-of time is returned; an explicit override wins over the fetched price; a per-listing fetch failure degrades gracefully (that holding unvalued with a reason, others still valued); quote-currency → AUD conversion applied — `closing_price` unit tests (AUD conversion + as-of, failures surfaced not zeroed, override/flag respected); handler tests in `portfolio` (live fills + as-of; override wins + per-listing failure degrades), `unrealised_gains` (live value + as-of; blanket failure degrades), and `performance` (live + as-of + override + failure + OVERALL unknown); `web::tests::live_valuation_ui_present`

## CGT decision support — parcel-selection optimiser and pre-sale what-if (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/cgt-keeping-records-shares.md` — parcel choice is the taxpayer's. Read-only; nothing persisted.)

- [x] Parcel-selection optimiser report: given listing, account, units, sale date, price (live-fetched default per the live-valuation rules; explicit override wins) → candidate strategies (minimise current-year gain, maximise discount-eligible proportion, harvest losses first, FIFO baseline), each with per-parcel allocations and gross gain / discountable split — `POST /portfolio/parcel-optimiser` (`reports/parcel_optimiser.rs`): the candidates are the open-parcels rows (so AMIT/E10, return-of-capital/G1, splits, and rollover carried dates/cost bases all flow through unchanged); each strategy allocates greedily in its preference order with FIFO tie-breaks (`min_gain` orders by per-unit *assessable* contribution — losses in full, discount-eligible gains at half weight; `max_discount` puts eligible gains before losses before non-eligible gains; `harvest_losses` biggest per-unit loss first, then FIFO); proceeds are spread by cumulative difference so they sum exactly to price × units; an unobtainable live price (with no explicit one) rejects with 422 and the reason rather than valuing nothing
- [x] Pre-sale what-if: net-capital-gain accepts a hypothetical disposal (units, proceeds, date, allocations or a named strategy) and returns the year's figures with and without it — dry run, no rows written; whole-of-income tax estimate stays out of scope — `POST /portfolio/net-capital-gain/what-if`: `db_net_capital_gain` split into `gross_buckets` + `net_years`, the hypothetical's realised-gains buckets injected into the disposal's tax year and the full loss-chaining walk re-run for both scenarios (the disposal year is ensured in both runs, so a year with no recorded activity still yields rows carrying the correct brought-forward chain); explicit allocations are validated like a real Sell's (open parcel of the listing, within remaining units, summing to the units), or derived from an optimiser strategy at the implied per-unit price `proceeds ÷ units`
- [x] Web UI: screens via the existing `REPORTS`/action config — the generic `viewReport` gained two config-driven capabilities used by both entries (no bespoke views): `params` (a submit-to-run form built from the shared field constructors) and `tables` (an object result rendered as titled tables per configured key, the comparison/summary/allocations layout)
- [x] Tests: each strategy's allocation choice; what-if leaves the DB untouched; API tests — `parcel_optimiser::tests::{fifo_takes_oldest_parcels_first, harvest_losses_takes_loss_parcels_then_fifo, min_gain_orders_by_assessable_contribution, max_discount_takes_eligible_gain_parcels_first, discount_window_edge_is_strictly_more_than_12_months, disposal_totals_split_gains_losses_and_sum_exactly, disposal_proceeds_sum_exactly_to_the_total}` + API tests (explicit/live/failing price, over-allocation and non-positive-unit 422s); `net_capital_gain::tests::{api_what_if_reports_the_year_with_and_without_the_disposal, api_what_if_on_an_empty_year_chains_earlier_losses, api_what_if_loss_offsets_recorded_gains, api_what_if_rejects_bad_allocations_and_modes}` (the first asserts unchanged trade/allocation counts and an unchanged report after the dry run); `web::tests::cgt_decision_support_ui_present`; `ato_examples::keeping_records_example_boris_optimiser_recommends_the_loss_parcel` (the harvest-losses candidate makes Boris's choice — all 1,500 from the 2024 $10 parcel, the $3,000 loss — the what-if previews the 2025 income year, and the holding is untouched)
- [x] Docs: `docs/API.md`, README Features — new "Parcel-selection optimiser" and "Pre-sale what-if" sections (incl. Response codes: the new 422 reasons, 200 covering report POSTs), two README Features bullets

## Compliance alert reports — wash sales and franking at-risk foresight (2026-06-10)

(REQUIREMENTS 2026-06-10; non-blocking, pattern: MIC validation / settlement coverage.)

- [x] Mirror the ATO wash-sale guidance (TR 2008/1 / current ATO page) into `docs/ato/` + `OVERVIEW.md` — read before implementing — `docs/ato/wash-sales.md` (TR 2008/1 legal-database print view + the QC 69938 "cleaning up dirty laundry" media release, retrieved 2026-06-11): no statutory window — Part IVA is a dominant-purpose test over the s 177D factors (Example 2's planned 24-hour round trip fails; Example 6's market-driven 3-day one survives) — so the report is advisory, never blocking
- [x] Wash-sale report: every loss-realising Sell with a Buy of the same listing within a configurable window (default 30 days), either side, across all holding accounts; writes never rejected — `POST /reports/wash_sales` (`reports/wash_sales.rs`, body `{"window_days": n}` optional, `n ≥ 1`): loss Sells come from `db_realised_gains` (capital_loss > 0 on any allocation — so the rollover/transfer exclusions and the cost-base pipeline apply unchanged), matched by date pattern against Buy/DRP trades of the listing in any account; provenance Buys that merely continue/relocate a holding (transfer-in, scrip replacement, demerger, inheritance) never match, rights-exercise and ESS Buys do
- [x] Franking at-risk foresight report: each dividend whose credits are denied by the 45-day walk (with the failing window/dates), plus a contemplated-sale mode reusing the holding-period walk; surfaced near the Sell flow in the UI — `GET /reports/franking_at_risk` + `POST /reports/franking_at_risk/what-if` (`reports/franking_at_risk.rs`): rows carry the qualification window (`ex_date`, `required_days` 45/90, `window_end`), entitled/disqualified units, and `credits_at_risk`/`credits_denied` with an `exempt_small_shareholder` status when the year's under-$5,000 exemption shields the failing walk. The candidates come from `franking::db_franked_dividends`, a loader extracted from (and now shared with) the tax summary, so the two cannot disagree; the what-if injects a hypothetical Sell into `franking::holding_period_test_with_sale` (the existing walk refactored over an event list) and reports each dividend whose denial would grow. The Sells list and Sell form link to the what-if, wash-sales, and parcel-optimiser screens (`sellForesightLinks`)
- [x] Tests: window edges; cross-account detection; denied-credit explanation matches the tax summary's denial — `wash_sales::tests::{db_window_edges_inclusive_either_side, db_window_is_configurable, db_repurchase_in_another_account_is_flagged, db_mixed_sell_with_a_loss_allocation_is_flagged, db_transfer_in_buy_is_not_a_reacquisition, db_gain_sell_is_not_flagged, db_buy_of_other_listing_is_not_flagged, api_post_wash_sales_defaults_and_custom_window}`; `franking_at_risk::tests::{db_denied_dividend_lists_failing_window_and_amounts, db_denied_amounts_match_the_tax_summary, db_small_shareholder_exemption_flagged_but_not_denied, db_what_if_shows_credits_a_contemplated_sale_would_cost, db_what_if_sale_after_window_end_is_safe, db_preference_listing_reports_90_day_window, api_get_franking_at_risk, api_post_what_if_and_validation}`; `web::tests::{wash_sales_report_ui_present, franking_at_risk_ui_present}`
- [x] Docs: `docs/API.md` (both reports), README Features — new "Wash sales" and "Franking at-risk" API sections (incl. the 422 reasons in Response codes and a cross-link from the tax summary's franking paragraph), two README Features bullets, `docs/ato/OVERVIEW.md` row for `wash-sales.md`

## Tax-return label mapping on the CSV exports (2026-06-10)

(REQUIREMENTS 2026-06-10.)

- [x] Verify the current year's myTax/paper labels from the ATO instructions and mirror the label reference into `docs/ato/` (+ `OVERVIEW.md`), recording which year's form the mapping targets — `docs/ato/tax-return-labels-2026.md` (retrieved 2026-06-11): the **Individual tax return 2026** instructions (FY2025–26, live on ato.gov.au since 30 May 2026) for questions 10, 11, 12, 13, 18, 20 and D7/D8, verified label by label from the paper-return question pages plus the myTax 2026 managed-funds SDS cross-reference (myTax shows the same labels, so one mapping serves both lodgment paths). Notable 2026 confirmations: 18A/18H/18V capital gains, 11S/11T/11U/11V dividends, 13U/13C/13Q/13R trust components (13C includes the attached credits), 20E/20M/20O foreign income with the A$1,000 FITO de-minimis restated, D7 (label I)/D8 (label H) with the LIC deduction at D8, ESS 12B/12D/12E/12F/12C/12A — the pre-2009 cessation label G no longer appears on the 2026 form. Question 10 (10L/10M) recorded ahead of the planned `interest_income` entity
- [x] Carry the mapping on the exports themselves (second header row or label column) without changing existing columns; document the full mapping in `docs/API.md` — `reports::export::csv_response` now writes a second header record from a per-report `CSV_ATO_LABELS` list (same length as `CSV_HEADER`, enforced by the non-flexible csv writer, so a column added without a label fails the request loudly); its first cell is `export::ATO_LABELS_MARKER` (`ato_labels_2026`), naming the form year on the export itself. Tax summary: direct labels (`11S + 11T`, `13U`, `13C`, `11U / 13Q`, `20E + 20M`, `20O`, `11V / 13R / 12C`, `12B`, `12A`, `D8` for the LIC deduction, `D7 / D8` for the deduction lines), `18 (working)` for the AMMA CGT inputs (the net-capital-gain export carries the final question-18 figures), empty for informational/derived columns. Net capital gain: `18A`, `18V`, `18V (prior year)` for the brought-forward column, `18H (component)` for the two gross-gain columns that sum to 18H, `18 (working)` for intermediate steps. Data rows and existing columns unchanged
- [x] Tests: export carries the labels; existing column assertions unchanged — `export::tests::{renders_headers_labels_and_rows_with_decimal_precision, empty_report_still_exports_both_header_rows, label_row_drift_is_an_error_not_misaligned_columns}` (plus the pre-existing header-drift test unchanged); `tax_summary::tests::db_ato_labels_align_with_their_columns` and `net_capital_gain::tests::db_ato_labels_align_with_their_columns` (each headline column's label under its column index); both reports' `api_export_returns_csv_with_expected_columns` keep their first-line `CSV_HEADER` assertion verbatim and additionally assert line 2 = the label row led by the marker; both empty-export tests assert the two header rows
- [x] Docs: `docs/API.md`, README — both export sections describe the second header row and carry the full per-column mapping table (with the 13C-includes-credits and 20E/20M notes); README's Tax-return CSV export bullet names the labels and the 2026 form target; `docs/ato/OVERVIEW.md` indexes the new mirror

## Expandable per-parcel CGT detail in the CGT reports (2026-07-12)
- [x] Realised Gains report: each disposal row carries a nested `parcels` breakdown (purchase trade,
      acquisition date, units, cost base, proceeds, gain/loss, discount-eligible) computed from the
      existing per-allocation loop in `compute_realised_gains` — the same figures already summed
      into the disposal's totals, not a new computation. New `ParcelDetail` struct
      (`reports/realised_gains.rs`), mirroring `parcel_optimiser::HypotheticalAllocation`'s field
      set. Covers both ordinary Sells and rights sales/lapses. Tested by
      `db_two_parcels_mixed_eligibility` (extended: asserts per-parcel figures and that they
      reconcile exactly to the disposal's totals) and `db_rights_sale_flows_into_the_report`
      (extended: asserts the rights-sale and ordinary-Sell parcel breakdowns)
- [x] Net Capital Gain report: each financial-year row carries its nested `disposals` (each with its
      own `parcels`) — `db_net_capital_gain` fetches the realised rows once, groups them by tax
      year, and attaches them after `net_years`; `gross_buckets` takes the realised rows as a
      parameter instead of re-fetching them. AMMA-attributed and CGT event E10/G1 gains have no
      parcel-allocation record and stay in the year's aggregate fields only. CSV export stays flat
      via a `NetCapitalGainYearCsv` projection struct (the `csv` crate rejects a nested `Vec`
      field); `CSV_HEADER`/`CSV_ATO_LABELS` and their drift tests are unchanged. Tested by
      `db_realised_and_amma_combined_in_one_year` (extended: asserts the year's `disposals` and
      their `parcels`, and that the AMMA-only gain has no disposal of its own) plus the existing
      passing CSV export tests (unchanged column count/order)
- [x] Web UI: `filterableTable` (`app.js`) supports a generic, composable `opts.expand` (a
      synchronous `row => childSpec` returning `{ rows, cols, opts }`; a child's own `opts.expand`
      recurses, giving the two-level Net Capital Gain nesting for free) rendering a leading ▸/▾
      toggle column and a full-width nested-table detail row, plus an "Expand all" / "Collapse all"
      control shown whenever a table supplies `expand`
- [x] Realised Gains and Net Capital Gain report views wired to the new expand option via `REPORTS`
      config (`config.js`): `expand: { key: 'parcels', … }` and the two-level
      `expand: { key: 'disposals', …, expand: { key: 'parcels', … } }`; `dataTable`/`buildExpand`
      (`app.js`) turn the declarative config into `filterableTable`'s `opts.expand`, pre-fetching
      every level's FK label maps up front (the expand callback must stay synchronous)
- [x] Parcel Optimiser and Pre-Sale What-If: folded their existing sibling `allocations` tables into
      the same inline expand-under-the-row UI (frontend-only, no backend change — both responses
      already carried the per-parcel allocations) — `expand: { from: 'allocations', matchOn:
      'strategy', … }` for the optimiser's `strategies` table, `matchOn: null` for the what-if's
      single `hypothetical` disposal. The what-if's `years` table needed an explicit `columns` list
      to exclude the flattened-in `NetCapitalGainYear.disposals` field (always empty on a
      scenario row), which surfaced a small general gap — `tables` entries now support an optional
      `columns` override, same as entity list configs
- [x] Docs: `docs/API.md` response shapes updated for `parcels` (realised gains) and `disposals`
      (net capital gain, noting the CSV export excludes it); README Features entries for the
      realised gains, net capital gain, parcel optimiser, and pre-sale what-if reports note the
      web UI drill-down. Tested end-to-end via `scripts/ui-check.sh` against a seeded Buy/Buy/Sell
      fixture — the toggle, Expand-all bar, and correct aggregate figures render on both reports —
      plus a new `web::tests::expandable_parcel_detail_ui_present` bundle-presence test

## Listing activity ledger report
Requirement (REQUIREMENTS.md 2026-07-13): one view of everything that ever happened to a listing,
in date order, ending in what is held now and what it is worth.
- [x] `POST /portfolio/activity` — `{ listing_id, price? }` → `events`, the listing's full history in
  chronological order: every trade labelled with its provenance (plain Buy/Sell, DRP reinvestment,
  rights exercise, buy-back, scrip exchange, demerger, worthless shares, ESS vest, inheritance,
  transfer network fee), transfers as one row (their group trades collapse into it), income,
  corporate actions, AMMA and ESS statements, rights sales, DRP enrolment periods, and
  listing-scoped investment expenses — each with signed units, a running `units_after` balance
  (splits/bonus issues re-base it), and the row's own money figure in AUD; all inputs read on one
  read transaction; 404 for an unknown listing — `reports/activity.rs`: the ledger assembles typed
  entity rows (`query_as` reuses each entity's own `FromRow` decimal parsing) on one `pool.begin()`
  with `FxRates` pre-loaded; trade provenance labels via the provenance columns + the transfer's
  `fee_sale_trade_id`; a trade's amount is its whole consideration converted with
  `FxOverride::from_trade` (the cost-base pipeline's convention), income by the tax summary's
  governing month, a rights sale with its `fx_rate` fallback
  (`reports::activity::tests::{db_ledger_is_chronological_with_running_balance,
  db_statement_rows_present_and_labelled, db_non_aud_trade_amount_converted_to_aud,
  db_unknown_listing_is_none, db_no_activity_is_empty_ledger, api_activity_unknown_listing_404}`)
- [x] `holdings` — the final holding summary per holding account (units held, cost base, market
  value): the portfolio-overview rows for the listing, live-priced by default with an explicit
  `price` override winning, degrading gracefully when no price is obtainable —
  `portfolio::db_holdings` gained a `db_holdings_on(conn, as_of)` split (mirroring
  `db_open_parcels_on`) so the summary reads on the ledger's own transaction; valuation mirrors the
  overview handler (`reports::activity::tests::{api_activity_with_price_values_summary,
  api_activity_live_values_summary, api_activity_without_price_degrades_gracefully}`)
- [x] Running balance reconciles: the last event's `units_after` equals the holding summary's total
  quantity (splits re-base, transfers net out, operation-created trades included) — same-date
  ordering puts a corporate action before the day's trades (a trade dated on a conversion date is
  already post-split, TD 2000/10)
  (`reports::activity::tests::{db_ledger_is_chronological_with_running_balance,
  db_same_date_split_applies_before_trade, db_transfer_collapses_to_one_row}`)
- [x] Web UI: Listing Activity report (params form + Activity/Holding-summary tables) through the
  generic config-driven report machinery, with the new columns classified/labelled — one `REPORTS`
  entry with `params` + two `tables`; `amount_aud` classified money / labelled "Amount (AUD)",
  `units_after` quantity / "Units after" (`web::tests::listing_activity_report_ui_present`)
- [x] Docs: API.md section (+ Response codes checked: 200/404 already covered generically, no new
  codes) and README feature bullet — API.md "Listing activity" under Portfolio reports documents
  the row kinds, field semantics, and the single-snapshot read; README gains the ledger bullet

## Report snapshots: provisional FX, catch-up generation, self-healing prices (REQUIREMENTS 2026-07-16)
The daily `report-snapshot` job fails all month for non-AUD holdings (no ATO monthly rate until
after month end) and both the price-import and snapshot jobs only target "the latest" date, so a
missed day is a permanent series hole. Strategy: flag-and-true-up provisional FX instead of
failing, and bounded catch-up windows in both jobs so late inputs delay a snapshot instead of
losing it. See REQUIREMENTS.md 2026-07-16 for full context.
- [x] Valuation-only FX fallback: a new explicit resolution mode in `infra/fx.rs` — when the valuation month has no imported rate, use the most recent earlier month's rate for that currency, at most 2 months back, else fail loudly as today. The result distinguishes a real-month rate from a fallback one (caller must know). Only valuation paths (snapshot generation, live-quote conversion) can invoke it — no tax calculation or FY report can reach a fallback rate — `fx::resolve_valuation_rate` / `FxRates::resolve_valuation_rate` return `ValuationRate { rate, provisional }` (bound: `VALUATION_FALLBACK_MONTHS = 2`); the strict `resolve_rate`/`pick_rate` path is untouched (and the pool-based `to_aud`/`resolve_rate` twins, now reached only by tests pinning both paths resolve identically, are `#[cfg(test)]`). Tests: `fx::tests::valuation_rate_*` (real month unflagged, ≤2-month fallback flagged, year-boundary crossing, >2 months → `MissingRate` naming the valuation month, AUD always final)
- [x] Live-quote conversion (`fetch_live_aud_prices`) uses the fallback: an early-month USD holding is valued (annotated as provisional) instead of erroring with a missing-rate reason — `LiveValuation` gains `fx_provisional`, carried onto the live-valued report rows (`fx_provisional` on overview/unrealised/performance/activity rows, serialised only when true) and rolled up in the web UI's "as at" line ("valued at a provisional FX rate"). Test: `portfolio::tests::api_overview_live_early_month_usd_is_valued_provisionally` (fallback month valued + flagged; a >2-month gap still degrades to `price_unavailable`)
- [x] `report_snapshots.provisional` flag (additive migration, no data loss, staleness triggers unaffected): set iff any conversion in the generation run used a fallback-month rate; regeneration with all real rates clears it; semantics distinct from `stale` (facts changed) — migration `0015_snapshot_provisional.sql`; `snapshot::generate` stores the flag and annotates the affected stored rows' `fx_provisional`. Test: `snapshot::tests::db_missing_month_rate_makes_snapshot_provisional_until_regenerated` (also pins that the rate import stales nothing — provisional, not stale); `db_rate_gap_beyond_fallback_bound_still_blocks` pins the loud-failure bound
- [x] `provisional` surfaced in the list/get/series API responses and the web UI (snapshot list + series/graph mark provisional points, as they do stale) — `SnapshotMeta`/`Snapshot`/`SeriesPoint` carry it; the Snapshots list badges `provisional` (stale wins the badge), the detail view explains it, the SVG series marks provisional points with a dashed ring. Tests: `snapshot::tests::api_generate_list_get_and_series` (flag in list/get/series JSON), `web::tests::report_snapshots_ui_present` (badge, hint, chart classes, CSS)
- [x] Snapshot job catch-up: each run generates every missing snapshot date in a bounded lookback window (from the last stored snapshot date, capped ~14 calendar days) up to the latest fully-valuable date, and regenerates stale or improvable-provisional snapshots in the window; a still-blocked date is skipped with its blocker surfaced (log + job failure detail) and retried on later runs — `run_snapshot_job` enumerates every date from the series' first stored snapshot (capped at `CATCHUP_LOOKBACK_DAYS = 14` before the latest; a fresh database starts at the latest date) so interior holes a blocked date left are retried too; stale and provisional window dates regenerate each run (a provisional date whose real rate is still missing just stays provisional). Tests: `snapshot::tests::db_job_catches_up_missing_dates_and_retries_blocked_ones`, `db_job_lookback_window_is_capped_and_regenerates_stale_in_window`
- [x] Price-import lookback: `run_collection` re-attempts, per held listing, every trading day in the last ~7 trading days whose stored row is missing or errored — not just the latest complete trading day; ok rows are never re-fetched (idempotent), no schedule changes — `COLLECTION_LOOKBACK_TRADING_DAYS = 7`, one provider call spanning only the days needing work. Tests: `closing_price::tests::collection_backfills_missing_and_errored_days_in_the_lookback`, `collection_skips_days_already_stored_ok` (no re-fetch on a filled window), plus the reworked failure/recovery/mismatch/crypto collection tests
- [x] RBA-import true-up: after a successful FX import that added new (currency, month) rows — the weekly `rba-fx-import` job and the manual `POST /rba_fx_rates/import` both — provisional snapshots whose valuation now resolves with a real rate are regenerated in that same run — shared `rba_fx_rate::true_up_provisional_snapshots` (no-op when nothing inserted) calls `snapshot::regenerate_provisional`; the manual endpoint's response carries the summary as `snapshot_true_up`, and the job fails loudly (import itself already committed) if a true-up date is blocked. Test: `rba_fx_rate::tests::api_import_regenerates_provisional_snapshots_in_the_same_run`
- [x] "Regenerate all" (API endpoint + web UI button on the snapshots screen): regenerates every stored snapshot date across the series; per-date blockers reported, unblocked dates still regenerate; single-date generation semantics reused — `POST /report_snapshots/regenerate_all` → `RegenerateSummary { regenerated, blocked: [{date, reason}] }` (200 even with blockers). Tests: `snapshot::tests::api_regenerate_all_and_provisional`, `web::tests::report_snapshots_ui_present` (button + endpoint in the bundle)
- [x] "Regenerate provisional" (API endpoint + web UI button): same shape, provisional snapshots only — the manual counterpart of the post-import true-up — `POST /report_snapshots/regenerate_provisional`, same summary shape and tests as above
- [x] Docs: `docs/SCHEMA.md` (`provisional` column), `docs/API.md` (flag in responses, the two regeneration endpoints, Response codes), README features (provisional-then-finalised snapshots) — plus the FX-conversion section's valuation-only-fallback rule, the Live-valuation provisional bullet, the price-import/report-snapshot job descriptions (API.md Jobs, README features, `schedule.cron` comments), and the Known-limitations intraday entry. Test: `doc_checks::provisional_snapshots_and_catchup_documented`

## Portfolio overview performance panel — graph, date range, and period attribution (REQUIREMENTS 2026-07-25)
The market-value/unrealised-gain graph lived on the Snapshots maintenance screen, which nobody
visits to see how the portfolio is doing; the Portfolio Overview screen (the app's landing page)
had no history at all, and no report answered "how did the portfolio do between two dates, and
why" — every existing report was point-in-time or FY-keyed. See REQUIREMENTS.md 2026-07-25 for
full context.
- [x] Extract the stored-price valuation path into `src/reports/valuation.rs` (`stored_valuations`, `ListingValuation`, `ValuationError`); `snapshot::aud_prices_for` becomes a thin adapter over it — behaviour unchanged, pinned by the pre-existing `reports::snapshot::tests` suite (all 15 tests still pass unmodified against the new adapter)
- [x] N/A — no separate `cash_income` extraction was needed: `reports::performance` already exposed a reusable, HTTP-independent `pub async fn db_performance(pool, prices, as_of)` (not gated behind the axum handler), so `period_performance` calls it directly at both endpoints and subtracts; no duplicated cash-income formula was introduced to extract in the first place
- [x] N/A — same finding: `performance::db_performance` was already the callable, fetcher-independent function the plan wanted extracted; the existing `performance_handler` is unchanged
- [x] New report `src/reports/period_performance.rs`: `POST /portfolio/period-performance`, `(from, to]` window, capital/FX/income breakdown that sums exactly to the period return (`capital_growth = total_return − fx_movement − income`, a residual, so the three always add up exactly), per-holding contributions, per-currency FX (with the rate pair used, omitted when listings in a currency resolved different pairs), informational `realised_capital_gain` (the tax figure, explicitly not part of the additive split), `provisional` flag, 422 on `from >= to` or a blocked valuation (missing/errored stored price, unfinal close, FX gap beyond the 2-month fallback) — `reports::period_performance::tests::*` (10 tests)
- [x] Rust tests: additivity, cross-check against `reports::performance`'s cumulative totals, AUD-only zero-FX, hand-computed USD rate-change FX, holding opened/closed mid-window, income-only period (all-return-is-income), provisional-rate propagation (period-level and per-currency), blocked-price and invalid-range 422s, null `total_return_pct` when opening value is zero; plus API tests via `oneshot` — `reports::period_performance::tests::*`
- [x] `src/web/chart.js`: moved `svgEl`/`seriesChart` out of `app.js` verbatim; added `presetRange` (1M/3M/6M/1Y/FY-to-date/all, computed from the series' own latest stored date not `today`, clamped to the earliest stored date) and `sliceSeries`; registered in `JS_MODULES` (`src/web.rs`, now 5 modules); node tests in `chart.test.js` (12 tests: every preset, the FY-boundary rule matching `domain::tax_year::tax_year_for`'s July-counts-as-next-FY convention, clamping, `sliceSeries` inclusivity/empty/missing-series) — `node --test 'src/web/*.test.js'` 36/36 green
- [x] Portfolio Overview screen gains the performance panel: range presets + custom from/to dates (resolved to the nearest actual stored snapshot dates before calling the report, so the summary always matches stored prices and the chart's own endpoints), a `.perf-summary` stat grid, a per-currency FX table, per-holding contributions in a collapsed `<details>` via `filterableTable`, and a provisional-FX warning banner; config-driven via a `performancePanel` key read generically in `viewReport` (inserted into all three of its `setMain` branches, not overview-specific code) — new money-column classifications added to `util.js`'s `COLUMN_KINDS` (`opening_market_value`, `closing_market_value`, `purchases`, `sale_proceeds`, `capital_growth`, `fx_movement`, `realised_capital_gain` as `money`; `rate_from`/`rate_to` as `rate`) so the new fields format through the shared table formatter rather than a bespoke view. Pinned by `web::tests::portfolio_overview_ui_present` and manually verified end-to-end: an ephemeral server on a temp DB, listings/trades seeded via the real HTTP API, closing prices and FX rates hand-inserted (the only way to seed prices without a live fetch), two snapshots generated, then a headless-Chrome DOM dump of `#/r/overview` — every rendered figure (opening/closing value, period return, capital growth, FX movement, per-currency rates, per-holding rows) matched hand computation exactly
- [x] Snapshots screen drops the chart card and its `/report_snapshots/series` fetch — kept only the generate/regenerate controls and the meta table; `config.js`'s snapshots `desc` no longer claims the time-series graph, pointing at the overview screen instead — pinned by `web::tests::report_snapshots_ui_present`
- [x] `web.rs` bundle assertions cover the panel view and the `/portfolio/period-performance` path — `web::tests::portfolio_overview_ui_present`, `web::tests::every_module_import_is_served`, `web::tests::js_test_files_are_not_served_and_every_module_is`
- [x] Docs: `docs/API.md` new `### Period performance` section (request/response, the `(from, to]` convention, the FX-attribution formula, the 422 catalogue entry), a Known-limitations entry (FX attribution is approximate for a holding traded inside the window — the residual lands in capital growth, and `capital_growth + fx_movement + income` still sums exactly to `total_return` regardless), README features (the panel on the overview screen, snapshots screen no longer claiming the graph), `src/infra/fx.rs`'s module doc comment + CLAUDE.md's `resolve_valuation_rate` allowed-callers sentence updated for the new caller, CLAUDE.md's web module list and pure-helper-testing sentence gain `chart.js`; no schema change (no new table/column) — `doc_checks::period_performance_panel_documented` pins the doc text
- [x] `scripts/ui-smoke.sh` gains an `#/r/overview` check for the performance panel — the closing-price/snapshot data needed to exercise the *populated* chart/summary can only be seeded via a live price fetch (there is no direct write path for `closing_prices`), which the network-free smoke check deliberately never does; the empty-series hint is what it asserts instead, with the populated case covered by `reports::period_performance`'s unit/API tests plus the manual `/verify` pass above
- [x] `cargo build`, `cargo test` (1121 passed), `cargo fmt --check`, `cargo deny check advisories`, `node --test 'src/web/*.test.js'` (36 passed), `scripts/ui-smoke.sh` all clean; manual end-to-end `/verify` pass on `#/r/overview` confirmed the range control, stat grid, per-currency FX, and per-holding table all render correctly and the breakdown sums to the period return on screen

## Snapshots screen: date-ranged regenerate-all (REQUIREMENTS 2026-07-25)
`POST /report_snapshots/regenerate_all` only ever re-ran dates that already had a stored snapshot
(`SELECT DISTINCT snapshot_date FROM report_snapshots`) — it could never create a snapshot for a
date that never had one, so a date backfilled with old closing prices still needed one-at-a-time
`POST /report_snapshots/generate` calls, and the Snapshots screen's button had no way to express a
range. See REQUIREMENTS.md 2026-07-25 for full context.
- [x] `default_regenerate_range(pool, now)` (`src/reports/snapshot.rs`): the default bulk-regen
  bounds — `from` = `MIN(date)` over Buy/DRP trades, `to` = `latest_snapshot_date` — both `None`
  when nothing has ever been held. `regenerate_all(pool, from, to, now)` takes optional bounds,
  defaulting either from this; `from` is clamped up to the first-ever-held date so an over-wide
  caller-given `from` can't spin through years of no-op days; `from > to` → `Unprocessable` (422).
  Walks every calendar day in range, keeping only dates `closing_price::db_held_listing_ids` finds
  something held on (the same guard the scheduled job uses), and regenerates them via the existing
  `regenerate_dates` — a date with no stored snapshot is generated for the first time, a stored one
  is force-regenerated regardless of its stale/provisional/fresh flags (kept as the reliable full
  repair, not narrowed to a catch-up window); blocked dates are still reported, not fatal. Tests:
  `snapshot::tests::db_regenerate_all_over_a_range_backfills_missing_dates` (backfills dates that
  never had a snapshot, clamps an over-wide `from`), `api_regenerate_all_accepts_a_date_range`
  (the `regenerate_range` endpoint's null/populated shapes, a narrowed range, the 422 for a
  backwards range), `api_regenerate_all_and_provisional` (updated for range semantics — a range
  spanning a weekend now also picks up the weekend's walked-back price)
- [x] `GET /report_snapshots/regenerate_range` → `Json<RegenerateRange>` for the UI to prefill the
  range boxes; `POST /report_snapshots/regenerate_all` takes an optional `{ "from", "to" }` body
  (`Option<Json<RegenerateBody>>`, so a bodyless POST still works and means the default range).
  Tests as above plus `web::tests::report_snapshots_ui_present`
- [x] Web UI (`src/web/app.js` `viewSnapshots`): two date inputs prefilled from
  `GET /report_snapshots/regenerate_range`, posted as the Regenerate all button's body; the result
  toast caps the blocked-date list at 5 with a `… and N more` tail instead of dumping a
  potentially long list. Pinned by `web::tests::report_snapshots_ui_present`
  (`/report_snapshots/regenerate_range`, `rangeFromInp`, `rangeToInp` present in the bundle)
- [x] `regenerate_dates` (the shared helper behind both `regenerate_all` and `regenerate_provisional`)
  logs each date's outcome as it completes — INFO `"snapshot regenerated"` on success, WARN
  `"snapshot regeneration blocked"` with the reason otherwise — carrying a running `done`/`total`
  count, so a long bulk run's progress is visible in the log file as it happens rather than only in
  the final JSON summary. Test: `snapshot::tests::db_regenerate_all_logs_progress_per_date`
  (`#[tracing_test::traced_test]` + `logs_contain`, exercising both the success and blocked lines
  and the done/total counters in one range)
- [x] Docs: `docs/API.md` (the new `GET` endpoint row, `regenerate_all`'s range/backfill body and
  semantics, its 422, the Web frontend paragraph), README (the snapshot bullet and the Web UI
  bullet) — `doc_checks::regenerate_all_date_range_documented`; no schema change, no change to the
  scheduled `report-snapshot` job's own 14-day catch-up window
- [x] `cargo build`, `cargo test` (1125 passed), `cargo fmt --check`, `cargo deny check advisories`,
  `node --test 'src/web/*.test.js'` (36 passed), `scripts/ui-smoke.sh` all clean;
  `scripts/ui-check.sh --seed demo '#/r/snapshots'` confirmed the two date boxes render prefilled
  (from = the fixture's first Buy date, to = the real latest fully-valuable date); a manual
  end-to-end HTTP pass against a fresh temp DB (one AUD listing, one Buy, prices backfilled for
  2026-06-01..08 only) confirmed a bodyless `regenerate_all` generated exactly those 8 never-before-
  snapshotted dates (24 stored rows = 3 reports × 8 dates) while reporting every later date blocked
  with an actionable "backfill it" reason, and that an explicit backwards range 422s

## Portfolio Overview range presets, remembered range, hide inactive holdings (REQUIREMENTS 2026-07-26)
Client-side-only change to the performance panel (`performancePanel`/`renderPeriodSummary` in
`src/web/app.js`, presets in `src/web/chart.js`). No schema or endpoint change.
- [x] Added 2Y and 3Y to the range-preset list (`presetRange` in `src/web/chart.js`), reusing
      `addMonths`; clamps to the earliest stored date like the existing presets. Test:
      `src/web/chart.test.js` (`presetRange: month-based presets end at the series' own latest
      date`, `a preset never precedes the series' earliest stored date`, extended with 2Y/3Y
      assertions)
- [x] The last-used preset is remembered across reloads via `localStorage`, defaulting to `all`
      when nothing is stored or the stored value isn't a known preset — new `loadPref`/`savePref`
      helpers in `src/web/util.js` (the app's first use of any client-side persistence; storage
      access is wrapped in try/catch so a throwing/disabled store degrades to "nothing stored"
      rather than breaking the view). `performancePanel` tracks the active preset in a closure
      variable, restores it on open (validated against `RANGE_PRESETS`), and highlights the
      matching button (`.range-control button.small.active`, `syncPresetButtons`). Applying a
      custom From/To range clears the remembered preset (`savePref(RANGE_PREF_KEY, null)`) instead
      of storing fixed dates, so the next load falls back to `all` rather than a stale range. Tests:
      `src/web/util.test.js` (`loadPref`/`savePref` round-trip, fallback on missing key, fallback on
      a throwing store, null/empty clears back to the fallback) and
      `web::tests::portfolio_overview_range_presets_and_activity_filter_present` (served-bundle
      wiring: `RANGE_PRESETS`, the `share-tracker.overview.range` key, `loadPref`/`savePref`,
      `syncPresetButtons`)
- [x] Added a default-checked "Hide holdings with no activity in this period" checkbox above the
      per-holding contributions table (`renderPeriodSummary`), filtering out holdings where
      `opening_market_value`, `closing_market_value`, `purchases`, `sale_proceeds`, and `income` are
      all exactly zero — `holdingHasActivity` in `src/web/util.js`, compared via the signed exact
      decimal-string helper `decStrEq` (handles `"0.00"`/`"-0.00"` spellings), never
      `Number()`/`parseFloat`. A hidden-count hint shows below the table, and a holding that was
      merely flat (unchanged value, no trades) stays visible — only holdings closed before the
      period even started are hidden. Checkbox state is remembered the same way as the range preset
      (`share-tracker.overview.hideInactive`). Tests: `src/web/util.test.js`
      (`holdingHasActivity`: all-zero row incl. `"-0.00"` → false, income-only → true, flat holding
      → true) and `web::tests::portfolio_overview_range_presets_and_activity_filter_present`
      (`holdingHasActivity`, the checkbox label text, and the `hideInactive` key present in the
      bundle)
- [x] Docs: README's Portfolio overview bullet (full preset list incl. 2Y/3Y, the remembered-range
      behaviour, the hide-inactive checkbox) and `docs/API.md`'s Web frontend and Period performance
      sections (the endpoint returns a row for every holding with any history unfiltered; the UI
      hides all-zero ones by default). New doc_checks pin:
      `doc_checks::overview_range_presets_and_activity_filter_documented`
- [x] `cargo build`, `cargo test` (1133 passed), `cargo fmt --check` all clean; `node --test
  'src/web/*.test.js'` (55 passed) clean. End-to-end verified against a real server (seeded demo
  fixture + synthetic closing prices spanning 2024-02 to 2026-07, snapshots generated) via headless
  Chrome driven over the DevTools protocol: a fresh load defaults to All; clicking 1Y persists to
  `localStorage` and highlights the button; reloading restores 1Y (correct From date, one year back
  from the latest snapshot) with the checkbox still unchecked from an earlier toggle; applying a
  custom range clears the remembered preset and the next reload falls back to All. Screenshots
  confirm the 2Y/3Y buttons render and the active preset is visibly highlighted.

## Annual tax report — printable per-year tax document (REQUIREMENTS 2026-07-26)
A year-selected, printable/archivable tax document (`src/reports/tax_report.rs`, `src/web/
taxreport.js`), distinct from the existing multi-year Tax Summary screen (unchanged).
Presentation/reconciliation only — no new tax math; every figure is sourced from
`domain::cost_base`, `reports::realised_gains`, `reports::net_capital_gain`, and
`reports::tax_summary`.
- [x] Itemised cost-base adjustments: `domain::cost_base::adjustment_detail` — a reporting-only
      sibling of `adjusted_cost_base` (not a change to `CostBase` itself, which stays on the hot
      path of five reports) returning one `CostBaseAdjustment` row per AMIT/return-of-
      capital/split-rebase adjustment, each with its date, human reference, per-unit figure, and a
      `capped` bit marking the row that first drives the running balance to nil (E10/G1). Fed by
      the new `entities::amit_adjustment::db_cost_base_reduction_detail` (the itemised sibling of
      `db_cost_base_reductions`). Tests (`cost_base.rs`):
      `itemised_amit_rows_sum_to_the_netted_reduction_including_the_floored_case`,
      `itemised_roc_and_split_rows_sum_to_the_netted_reduction` — itemised rows sum exactly to
      `CostBase`'s netted `amit_reduction`/`roc_reduction`, including the floored case
- [x] `src/reports/tax_report.rs`: `GET /reports/tax-report/years` and `POST /reports/tax-report`
      (body `{ tax_year }`), registered in `reports::router()`. The core financial sections (the
      disposal schedule, the CGT summary, the year's `TaxYearSummary` line) read on one
      `pool.begin()` transaction, folding in `realised_gains::db_realised_gains_on` and the new
      `tax_summary::db_tax_summary_on`/`net_capital_gain::db_cgt_summary_year`; the completeness
      cross-checks and franking-entitlement detail deliberately read their own snapshots (advisory
      notes/drilldown rows alongside a total computed elsewhere — documented in the module doc). An
      out-of-range year returns a zeroed document, not an error. Test:
      `empty_year_returns_zeroed_document_not_error`
- [x] Completeness section: `amma_missing` — a new holdings-based check (a simple net-Buy/DRP-minus-
      Sell walk per AMIT listing, not cost-base aware) that fires for a fund held during the year
      with *no AMMA statement and no cash rows at all* — the gap the existing cash-driven
      `amit_cash_cross_check` documents it cannot catch — plus that report's and `e4_cross_check`'s
      existing alerts filtered to the year. Non-blocking; `complete` true only when all three are
      empty. Tests: `amma_missing_is_holdings_based_and_clears_once_entered`,
      `amma_missing_ignores_a_listing_not_held_during_the_year`
- [x] Trading activity section: per-parcel disposal detail grouped by listing (buy date/price,
      adjusted cost base with the itemised adjustment rows nested under it, sell date/price,
      gain/loss, discounted gain/loss, units, brokerage/GST both sides, contract note references,
      acquisition provenance via `activity::trade_event` — made `pub(crate)` — and native-currency/
      buy-and-sell-month-rate detail for a non-AUD parcel), with per-listing and grand totals
      computed server-side in `Decimal`. Test: `disposal_figures_equal_realised_gains_for_the_same_year`
- [x] Gain/loss summary section: `net_capital_gain::CgtSummaryYear` / `db_cgt_summary_year` — the
      ATO worksheet layout (short-term less losses; long-term split into the grossed-up AMMA
      discount-distribution component and everything else, less losses, less the 50% concession).
      `GrossBuckets` gained one field (`amma_discount_grossed_up`) to carry the split through the
      existing `gross_buckets`/`net_years` pipeline unchanged — no second netting implementation;
      `NetCapitalGainYear`'s public fields and CSV export are untouched. Test:
      `cgt_summary_reconciles_to_net_capital_gain_year`
- [x] Income section: Trust (+ full AMMA statement component detail), Dividend (each row carrying
      its `franking_status` — `entitled`/`denied`/`exempt_small_shareholder` — from
      `franking_at_risk`), Foreign, Interest, ESS, and Deductions detail, every AUD figure converted
      via `tax_summary`'s own `aud_field`/`aud_label` helpers (made `pub(crate)`, not re-implemented).
      Test: `income_sections_sum_to_tax_year_summary`
- [x] Overall tax summary section: the year's `TaxYearSummary` fields paired with their ATO labels,
      reusing `tax_summary`'s `CSV_HEADER`/`CSV_ATO_LABELS` (made `pub(crate)`) zipped together —
      one source of truth for the labels, shared with the CSV export; `db_tax_summary` factored into
      a `db_tax_summary_on(conn)` so the single-year read doesn't re-run the whole multi-year
      aggregation on its own transaction
- [x] Web UI: new `src/web/taxreport.js` module (added to `JS_MODULES` in `src/web.rs`, hard-coded
      length bumped 6→7), a `custom: 'tax-report'` REPORTS entry in `config.js`, a year dropdown +
      Generate button (nothing runs until pressed), a Print/Save-as-PDF button (`window.print()`).
      Renders plain semantic `<table>`s, deliberately not through `filterableTable` (its filter row,
      sort indicators, and 50-row pager have no business in a print document — the pager would
      silently print only the first page); the exception is recorded in CLAUDE.md's web-frontend
      module-graph description. Test: `web::tests::annual_tax_report_ui_present`
- [x] New `@media print` block in `src/web/style.css` (none existed before): hides
      nav/menus/toast/the year-select toolbar, drops `th`'s sticky positioning, repeats table
      headers per page (`thead { display: table-header-group }`), forces white background/black
      text, avoids breaking a table row/section across a page. Test:
      `web::tests::annual_tax_report_print_styles_present`
- [x] Docs per the standard sync rule: `docs/API.md` (a new "Annual tax report" section — both
      endpoints, response shape, the holdings-based completeness rule and why it's non-blocking, the
      "every figure is sourced from the existing pipelines" note — plus the Web frontend section's
      module table and report list), README's Features list (a new bullet) and Web UI bullet,
      CLAUDE.md's web-frontend module-graph description (the `filterableTable` exception and
      `taxreport.js`/`nav.js` added to the file list)
- [x] `cargo build`, `cargo test` (1144 passed), `cargo fmt --check`, `node --test
  'src/web/*.test.js'` (55 passed), and `scripts/ui-smoke.sh` (exit 0, "all routes rendered") all
  clean; `cargo deny check advisories` clean (no dependency changes). Manually verified end-to-end
  against a live scratch server: seeded an AUD listing with a discount-eligible disposal and an
  AMIT listing with no AMMA statement — `POST /reports/tax-report` returned the correct itemised
  cost base ($8010.945 initial → $3204.378 adjusted for the 40/100-unit allocation), proceeds
  ($4389.055), gain ($1184.677), and 50%-discounted gain ($592.3385), matching `realised_gains` and
  `net_capital_gain` exactly; `completeness.amma_missing` correctly flagged the AMIT listing (held,
  no cash rows, no statement — the gap the existing cash-driven check misses) and cleared once an
  AMMA statement was entered. `scripts/ui-check.sh --seed demo '#/r/tax-report'` confirmed the
  initial screen (year dropdown populated from real data, Generate/Print controls, nav highlighting
  the new Reports → CGT & tax entry) renders correctly in headless Chrome.

## Annual tax report: subtotal/total figures showed raw Decimal precision (2026-07-26 follow-up)
User feedback: several fields — most visibly the disposal subtotal/total lines — rendered with far
more decimal places than a money figure needs (e.g. `592.33850`, `3204.378`). `taxreport.js` builds
most money cells through the shared `moneyTd`/`moneyEl` helpers (which round through `numericDisplay`
the same way every other screen does), but the subtotal/total paragraphs and two alert messages were
built as plain string concatenation using `cellText` — which prints a `Decimal` verbatim, and this
report's totals routinely carry 3+ places from pro-rated brokerage/GST and the 50% discount halving.
- [x] Added `moneyText(value)` (the plain-string counterpart to `moneyEl`, same `numericDisplay`
      rounding) and used it in the disposal subtotal/total lines and the two completeness alert
      messages (`amit_cash_alerts`/`e4_alerts`) that had been using `cellText` on a money figure
- [x] `genericTable` (the income section's generic renderer) only money-formatted `_aud`-suffixed
      columns; the AMMA/trust `tax_deferred_amount`/`tax_free_amount` fields are native-currency
      (informational, never AUD-converted) but still cent figures — added an explicit
      `EXTRA_MONEY_COLUMNS` list so they round too
- [x] Verified: `node --check src/web/taxreport.js`, `cargo build`, `cargo test` (1144 passed),
  `cargo fmt --check`, `node --test 'src/web/*.test.js'` (55 passed) all clean; confirmed
  `numericDisplay` rounds the actual report figures correctly (`1184.677` → `1,184.68`,
  `592.33850` → `592.34`, `3204.378` → `3,204.38`, `80.10945` → `80.11`, full value kept on hover)

## Attachments index report (2026-07-26)
User request: attachments could only be seen one owner at a time (the per-owner
`#/attachments/<owner_field>/<owner_id>` view reached from a row's Attachments action) — no way to see
the whole document register across the portfolio, with a link to download or view each file.
- [x] `GET /reports/attachments` (`src/reports/attachments.rs`) lists every stored attachment LEFT
      JOINed out to its owning activity — `owner_type` (Trade/Income/AMMA statement/ESS
      statement/Interest income/Corporate action), `owner_field` (the matching `?<field>=` query key),
      `owner_id`, a human `owner_description` (e.g. "Buy on 2024-05-01"), and `listing_id` (null only
      for interest income, which has no listing) — alongside the existing filename/content_type/
      byte_size/uploaded_at metadata, newest upload first; tests
      `reports::attachments::tests::db_trade_owned_attachment` / `db_income_owned_attachment` /
      `db_amma_owned_attachment` / `db_ess_owned_attachment` /
      `db_interest_income_owned_attachment_has_no_listing` / `db_corporate_action_owned_attachment` /
      `db_orders_newest_upload_first` / `db_empty_when_no_attachments`, `api_get_attachments_report`
- [x] `GET /attachments/{id}/content` gained `?disposition=inline` (default stays `attachment`) so a
      file can be viewed in place rather than downloaded — both forms now also carry
      `X-Content-Type-Options: nosniff`; an unrecognised value is `422`; test
      `entities::attachment::tests::api_download_disposition_controls_content_disposition`
- [x] Report tables gained a generic `rowActions` mechanism (`dataTable`/`config.js`, mirroring the
      entity list's `rowActions`) since no report previously had per-row link actions; the Attachments
      report config wires Download / View (new tab, `?disposition=inline`) / Record (link to the
      owning record's own per-owner attachments view) — pinned by `web::tests::attachments_report_ui_present`
- [x] Docs: `docs/API.md` (`?disposition=inline` on the download endpoint, the new `### Attachments
      index` report section, the 422 case, the web-frontend report list) and a README feature bullet
      updated in the same change
- [x] Verified: `cargo build`/`cargo test` (1156 passed, warning-free), `cargo fmt --check` and
      `cargo deny check advisories` clean (no dependency changes), `node --test 'src/web/*.test.js'`
      (55 passed), `scripts/ui-smoke.sh` (added `#/r/attachments`, asserting the heading and the
      "No records." empty state — the demo fixture seeds no attachments, since seeding is JSON PUTs
      and an upload is multipart; confirmed rendered DOM by hand: heading, description, empty state)
      all clean


## A corporate action dated in the future is applied to today's holdings (SCENARIOS E-14)
(SCENARIOS.md section E verification pass, 2026-08-16. `domain::open_parcels::load(conn, None)`
resolves its cutoff with `as_of_or_open` (`src/domain/open_parcels.rs:112`), i.e. the `9999-12-31`
sentinel, so the *live* view means "every recorded fact" rather than "everything up to today". A
split or return of capital recorded ahead of its effective date — normal practice, the terms are
announced weeks before they take effect — is therefore already in force in every report built on
that call, while the as-of-dated reports correctly ignore it.)
- [x] E-14 — reproduced: Buy ×100 on 2023-01-10, `ShareSplit` 2-for-1 dated **2030-03-01**.
  `GET /portfolio/open-parcels` and `POST /portfolio/overview` report **200 units** (market value
  $2,000 at $10) today, in 2026; `POST /portfolio/unrealised-gains` for the same day reports **100**
  ($1,000). Two reports, one database, one day, two answers
- [x] E-14b — the same with a `ReturnOfCapital` of $1.00/unit dated 2030-03-01: open parcels report
  `return_of_capital_reduction: 100.00` and `remaining_cost_base: 900.00` today, the overview's
  `total_cost_base` follows, and the **parcel optimiser** (`POST /portfolio/parcel-optimiser`,
  `src/reports/parcel_optimiser.rs:109`) prices a contemplated sale off the reduced $9.00/unit,
  overstating the gain on every candidate strategy. Unrealised gains still show $1,000
- [x] The write paths are consistent with the *correct* reading — a Sell entered today is validated
  and costed against the pre-split basis — so it is only the live read that disagrees, which is what
  makes it silent
- [x] Fix shape: `load(conn, None)` (and `portfolio::db_holdings(pool, None)`,
  `open_parcels::db_open_parcels`) should bound at today rather than at the sentinel, so "live" means
  "as at today" everywhere; a future-dated fact then appears when it takes effect.
  **Decided 2026-08-16 (Evan): bound everything at today** — trades as well as corporate actions, one
  rule rather than a carve-out (a future-dated trade is nearly always a typo, and it will surface on
  its own date). Watch what else keys off the sentinel: `infra::date::as_of_or_open` is shared, so
  change the callers rather than the helper, and check the snapshot/valuation paths still pass their
  own explicit dates
- [ ] Alternative if the bound is unwanted: refuse a corporate action dated after today at write time
  — but that removes a legitimate entry (recording the terms on announcement), so bounding the read
  is the better half

**Fixed 2026-08-16 as decided — the live read is bounded at today, nothing is refused at write time**
(the alternative above stays rejected: recording terms on announcement is the intended workflow).
- [x] `infra::date` gained `today()` and `as_of_or_today` beside `as_of_or_open`, which is untouched
      and keeps its "every recorded fact" meaning for the reads that want it (the allocations behind
      an FY-keyed report, `closing_price::HeldTimeline::held_listing_ids`, the AMIT reduction events)
      — the caller changed, not the shared helper, exactly as the fix shape asked. Its doc comment
      now points at the sibling so the next reader picks the right one; tests
      `infra::date::tests::none_is_today_for_a_live_view` / `some_passes_through_the_live_resolver`
- [x] `domain::open_parcels::load` resolves `None` to today **once**, at the top, and passes the
      result on as `Some(cutoff)` to every bound below it (the parcel SELECT, `db_units_sold`,
      `amit_adjustment::db_cost_base_reduction_events`, `cost_base::Held::AsAt`,
      `split_adjusted_quantity`). One date for the whole function, so the live view is *identical*
      to the as-at-today view rather than merely similar — and every view built on the loader
      (portfolio overview, open parcels, the parcel optimiser and the pre-sale what-if through it,
      the listing-activity holdings block) is fixed by the one change, with no report-side edits
- [x] Tests: `domain::open_parcels::tests::the_live_view_ignores_a_future_dated_corporate_action`
      (E-14/E-14b at the loader — a future split and a $1.00/unit return of capital leave 100 units
      at $1,000 today, `load(None) == load(Some(today))`, and both take effect on their own date),
      `the_live_view_ignores_a_future_dated_trade` (the no-carve-out half: a future-dated Buy isn't
      held yet and a future-dated Sell hasn't consumed its parcel yet), and
      `the_live_reports_agree_with_todays_unrealised_report_on_a_future_action` (the finding as
      stated — overview, open parcels, parcel-optimiser candidates and the unrealised report for
      today all report 100 units / $1,000). The future dates are computed from `today()`, not
      hardcoded, so the tests can't rot into the past
- [x] Snapshot and valuation paths re-checked as the fix shape asked: `reports::snapshot` values a
      day through `portfolio::db_holdings(pool, Some(date))` and
      `unrealised_gains::db_unrealised_gains(pool, date)`, and `period_performance` through
      `valuation::held_markets(pool, Some(date))` — all explicit, none relying on the sentinel
- [x] Docs: `docs/API.md` gained a `### As-at date` subsection under Portfolio reports (the shared
      convention, beside FX conversion): an undated report is the position as at today, a
      future-dated trade or corporate action is recorded but not in force, trades are bounded the
      same way rather than carved out, and the realised/FY-keyed reports deliberately are *not*
      bounded (a disposal reports in its own financial year). Overview and Open parcels say "as at
      today" and link to it; pinned by `doc_checks::as_at_today_convention_documented`
- [x] Verified: `cargo build` and `cargo test` (1471 passed, warning-free — the 1467 that passed
      before the change all still pass, so nothing depended on the sentinel reading),
      `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean

## A duplicated corporate action is silently compounded (SCENARIOS E-03, E-15)
(SCENARIOS.md section E verification pass, 2026-08-16. Two actions of the same type, listing and
date are two independent events to every reader: `db_return_of_capital_events` and
`db_share_split_events` load both, and the pipeline sums / multiplies them.)
- [x] E-03 — two identical `ReturnOfCapital` rows ($0.50/unit, same date, same listing) reduce a
  100-unit parcel by **$100.00**, not $50.00
- [x] E-15 — two identical 2-for-1 `ShareSplit` rows on one date turn 100 units into **400**
- [x] Both are plausible double entries (a re-submitted form, a re-imported statement), both restate
  every cost base and quantity of the listing, and nothing — not the health report, not any
  cross-check — mentions it. Genuine same-day pairs exist in principle (two tranches of a capital
  return), so a hard uniqueness constraint would be wrong; the fit is a health-report warning naming
  the duplicated (listing, type, date), or a confirm step on the second write
- [x] **Decided 2026-08-16 (Evan): a health-report warning** — one row per duplicated
  (listing, action type, date), non-blocking, so a genuine same-day pair stays enterable. (A UI
  confirm step on the second write, and accepting it as a non-issue, were both considered and
  rejected.) `reports::health` gains the check and `docs/API.md`'s health section the field

**Resolution (2026-08-16): a `reports::health` warning, as decided — non-blocking, no constraint.**

`GET /reports/health` gained `duplicate_actions`: one row per (listing, action type, date) carrying
more than one corporate action, newest first, as
`{ listing_id, ticker, action_type, date, action_count, action_ids }` — the ids ascending, so the
surplus row is opened and deleted without a search. Grouped in SQL on the report's own read
transaction beside the other freshness reads (it is one small aggregate over `corporate_actions`,
not a per-listing walk like `unpriced_days`). Every action type is covered, not only the two that
compound silently: a duplicated `BuyBack` or `Demerger` is the same double entry, and the reader is
the one who knows whether the pair is real.

The web UI's cross-view banner names it — type, ticker, date and ids, with "each is applied
separately; delete the duplicate unless both are real" — and links to Corporate Actions beside the
existing Jobs and Closing Prices links, so the warning is visible from any screen rather than only
when the report is fetched. Verified end-to-end through `scripts/ui-check.sh` against a seeded pair.

`docs/API.md` states the field, why the effect compounds (both worked figures from E-03 and E-15),
and — the recorded decision — that this is deliberately *not* a uniqueness constraint, since a
genuine same-day pair exists in principle; it also names the way out (delete the surplus action,
`422` while a trade still references it). The README's monitoring bullet and the banner paragraph
list it alongside the other health surfaces.

Tests: `reports::health::tests::duplicated_corporate_actions_are_reported_with_their_ids` (E-03 and
E-15 together, newest first, with the ids),
`actions_differing_in_listing_type_or_date_are_not_duplicates` (the key is all three, so ordinary
independent events stay silent), `three_identical_actions_are_one_row_counting_three`,
`empty_database_reports_nothing_stale` (extended), and `web::tests::health_banner_ui_present`.
Full suite 1481 passed / 0 failed.

## The AMIT cash cross-check ignores the holding account (SCENARIOS F-03, F-08)
(SCENARIOS.md section F verification pass, 2026-08-16. The report's "covered" set is keyed
`(listing_id, tax_year)` (`src/reports/amit_cash_cross_check.rs:68`), while a registry issues one
AMMA statement **per holder account** — which is exactly why `amit_adjustments` are constrained to
the statement's own account and why generation narrows to it.)
- [x] F-08 — reproduced: VDHG cash row in account 1 for FY2025 is flagged while no statement
  exists; entering a statement for the same fund and year **in account 2** clears the flag,
  although account 1's income is still unattributed and still excluded from the tax summary
- [x] The fix is to key the covered set by `(listing_id, holding_account_id, tax_year)` and report
  the account on the alert (F-03 confirms the two-account case is otherwise handled correctly
  throughout: generation, the adjustment cross-check and the cost-base reports all narrow by
  account)

**Resolution (2026-08-16): coverage keyed by `(listing, holding account, financial year)` in both
AMMA-coverage checks.**

`reports::amit_cash_cross_check` now builds its covered set from
`(listing_id, holding_account_id, tax_year_end_date's year)` and aggregates the cash rows per
`(ticker, listing, FY, account)`, so a statement issued for one holder account clears only that
account's row. `AmitCashAlert` carries the new `holding_account_id`, and its doc comment says why
the account is part of the key (the same reason an AMIT adjustment may only touch its statement's
own account).

The annual tax report's holdings-based `amma_missing` check had the identical blindness and was
fixed with it — it gates the report's `complete` flag, so leaving it keyed by listing alone would
have gone on calling a year complete while one account's attribution was missing. Its net-units
walk and its covered set are now keyed by `(listing, account)`, `AmmaMissingAlert` carries
`holding_account_id`, and the printed completeness lines name the account ("… for AMT in account
#2"), which two accounts of one fund would otherwise render as two identical sentences.

Docs: the [AMIT cash cross-check](../docs/API.md#amit-cash-cross-check) section states the per-account
rule, its reason, and the new response field; the annual tax report's completeness bullet says the
same for `amma_missing`; the README's cross-check bullet and the web UI's report description follow.

Tests: `reports::amit_cash_cross_check::tests::db_amma_in_another_holding_account_does_not_clear_the_flag`
(F-08 exactly) and `db_each_holding_account_is_flagged_and_cleared_on_its_own` (two accounts, two
alerts, cleared one at a time), `reports::tax_report::tests::amma_missing_asks_each_holding_account_for_its_own_statement`,
and `doc_checks::amma_coverage_is_documented_as_per_holding_account`. Full suite 1491 passed / 0
failed.

## Two AMMA statements for the same fund and year are silently double-counted (SCENARIOS F-06)
(SCENARIOS.md section F verification pass, 2026-08-16. `amma::db_upsert`
(`src/entities/amma.rs:192`) validates only that `tax_year_end_date` is a 30 June date — nothing,
in Rust or in the schema, stops a second statement for the same `(listing_id, tax_year_end_date,
holding_account_id)`. Every reader then counts both: the tax summary's `amma_*` lines
(`src/reports/tax_summary.rs:471`), the net-capital-gain report's gain buckets
(`src/reports/net_capital_gain.rs:684`), and — because the duplicate-parcel UNIQUE index of
migration 0022 is per *statement* — a second generated adjustment set reducing every parcel a
second time. This is the AMMA counterpart of E-03's duplicated corporate action, which was closed
with a health warning in `e15e60a`.)
- [x] F-06 — reproduced: VDHG, Buy ×1000, statement #1 for FY2025 (`other_income` 300,
  `cgt_discount_gains` 100, `cost_base_adjustment` 0.20), then the fund's **amended** statement
  entered as #2 (350 / 120 / 0.25) instead of editing #1. Both `PUT`s answer `204`; generation
  succeeds on each. Result: tax summary `amma_other_income` 650 and `amma_cgt_discount_gains` 220,
  net capital gain `discount_eligible_gains` 440 (both statements grossed up), and the parcel's
  `amit_cost_base_reduction` 450 (200 + 250) — every figure the sum of the original and its
  replacement
- [x] Nothing surfaces it: the AMIT adjustment cross-check reconciles both sets (each matches its
  own statement's `units_held`), the AMIT cash cross-check sees the year as covered, and
  `/reports/health` has no equivalent of `duplicate_actions` (`src/reports/health.rs:265`)
- [x] **Decided 2026-08-16 (Evan): a health-report warning, like E-03** — non-blocking, no
  uniqueness constraint, so an amended statement stays enterable beside the original while the two
  are compared. The options weighed were: (a) refuse a second statement for
  the same (listing, year, holding account) at write time — safest, but an amended statement then
  has to be entered by editing the original row, and the audit trail of what the first one said
  lives only in `row_history`; (b) a non-blocking `duplicate_amma_statements` health warning, the
  E-03 precedent, keeping both rows enterable; (c) document it. Note the account is part of the
  key either way (F-03 shows two accounts legitimately have two statements for one fund-year)

**Resolution (2026-08-16): a `reports::health` warning, as decided — non-blocking, no constraint.**

`GET /reports/health` gained `duplicate_amma_statements`: one row per (listing, financial year,
holding account) carrying more than one AMMA statement, newest year first, as
`{ listing_id, ticker, tax_year, holding_account_id, statement_count, statement_ids }` — the ids
ascending, so the superseded row is opened and deleted without a search. Grouped in SQL on the
health report's own read transaction, beside `duplicate_actions`, which it is modelled on
line-for-line.

The holding account is part of the key, not an afterthought: a fund held in two accounts
legitimately has two statements for one year (SCENARIOS F-03), so keying on (listing, year) alone
would have made the warning fire on correct data. Grouping is on `tax_year_end_date` itself — the
column is a 30 June date, enforced at write time — and the reported `tax_year` comes from
`domain::tax_year::tax_year_for`, so the row reads in the same FY terms as every other report.

The web UI's cross-view banner names it — count, ticker, FY, ids, with "every figure is counted
once per statement; delete the superseded one unless both are real" — and links to AMMA Statements
beside the existing Jobs, Closing Prices and Corporate Actions links. Verified end to end against a
running server: two FY2025 statements for one account produce the banner, and deleting the
superseded one clears it.

`docs/API.md`'s health section states the field, why it compounds (both the income/gains
double-count and the second generated adjustment set, since the one-adjustment-per-parcel UNIQUE
index is per statement), the recorded decision that this is deliberately *not* a uniqueness
constraint — a registry change mid-year or a fund merger leaves two genuine part-year statements
for one account — and the way out (delete the superseded statement, `422` while its AMIT
adjustments are still stored). The README's monitoring bullet lists it alongside the duplicated
corporate action.

Tests: `reports::health::tests::duplicated_amma_statements_are_reported_with_their_ids` (F-06's
amended statement, two fund-years, newest first, with the ids),
`statements_differing_in_listing_year_or_account_are_not_duplicates` (the key is all three parts —
the two-account case stays silent), `three_statements_for_one_fund_year_are_one_row_counting_three`,
`empty_database_reports_nothing_stale` (extended), and `web::tests::health_banner_ui_present`.
Full suite 1494 passed / 0 failed.

## An AMMA statement for a year with nothing held at 30 June cannot be generated, and its hand-entered set is flagged forever (SCENARIOS F-04, F-17, F-25)
(SCENARIOS.md section F verification pass, 2026-08-16. `db_generate` refuses with `NothingHeld`
when no parcel of the listing is open at the statement's `tax_year_end_date`
(`src/entities/amit_adjustment_generation.rs:141`), and the cross-check's coverage rule compares Σ
of the adjustment quantities against the statement's `units_held`
(`src/reports/amit_adjustment_cross_check.rs:207`). Both are right for the case they were written
for — a statement whose parcels have not been entered yet — and both misfire on the *correct*
holding that was fully sold, or transferred out, during the statement's year. The reduction itself
is right once entered by hand: `AmitReductionEvent::reduction_for_units` spills a whole-parcel row
onto the units sold during the year, which is what LCR 2015/11 para 13 requires.)
- [x] F-04 — reproduced: Buy ×1000 Aug 2024, sold in full 1 Mar 2025, FY2025 statement stating 0
  units held and 0.20 per unit. `POST /amma_statements/1/generate_adjustments` → `422` "no parcels
  of the statement's listing were held in its holding account at the statement's year end — **enter
  the missing trades first**", which is the one thing the user must not do here: the trades are all
  entered and correct
- [x] The hand-entered row is accepted (`PUT /amit_adjustments/1` with `quantity` 1000 → `204`) and
  reduces the sale's cost base correctly (49,800 from 50,000 — the sale's gain rises by exactly
  1000 × 0.20). But the cross-check then reports the statement forever: "adjusted units 1000 do not
  match the statement's units held 0 (difference +1000) — a parcel is missing, duplicated, or
  covered for the wrong quantity". An honest, complete entry cannot be made to reconcile
- [x] F-25 shows the same path is the *normal* one for the year of sale: a multi-year holding sold
  in November has its FY-of-sale AMMA arrive the following September, and that statement always has
  0 units held. F-17 hits it from the other side: after a mid-year transfer, the sending account's
  statement has nothing open in that account at 30 June
- [x] **Decided 2026-08-16 (Evan): option (b)** — keep the refusal, re-word it, and teach the
  coverage check about units disposed of during the statement's year. The options weighed were:
  (a) let generation cover parcels held *during* the year when
  none is open at year end — one row per parcel the listing had open at any point in the FY, each
  covering the units it held (this is a real extension: the row quantity for a partly-sold parcel
  would have to be the units held during the year, not the units remaining); (b) keep the refusal
  but re-word it, and teach the coverage check that a statement stating fewer units than were
  adjusted is expected when the difference is units disposed of during the statement's year;
  (c) document the manual path. (b) is the smaller change and fixes both misfires

**Resolution (2026-08-16): option (b) — the refusal re-worded, and the coverage check given the
year's disposals as an allowance.**

Generation still derives its set from the parcels open at the statement's `tax_year_end_date` and
still refuses when there are none: writing an empty set would hide the two quite different
situations that reach it. What changed is that the 422 now names **both** of them — "if trades are
missing, enter them and run this again; if the holding was sold or transferred away during the
year, the statement still adjusts the units it covered, so enter one AMIT adjustment by hand
against each parcel those units came from". The old ending ("enter the missing trades first") was
the one instruction a user with a correct, closed holding must not follow.

The cross-check's coverage rule is no longer an equality against `units_held` but a band:
`units_held ..= units_held + the units of the adjusted parcels disposed of during the statement's
year` (both bounds inclusive, and both terms re-based into the year-end unit basis, so a split
still cannot false-positive). Below the band a parcel is missing — the old message, unchanged;
above it, a new message names the excess and what the ceiling was made of. The allowance is the
same rule the cost-base pipeline already applies: a row may cover units sold during the year
because s 104-107B makes the adjustment just before the end of the income year *or just before a
relevant CGT event* (LCR 2015/11 para 13). For a holding sold out or transferred away mid-year
those are the only units there are, so the honest hand-entered set now reconciles instead of being
flagged forever. Disposals are counted per parcel, not per row, so a duplicated parcel cannot widen
the band and mask itself.

Verified end to end against a running server on F-04's own reproduction: Buy ×1000 Aug 2024, sold in
full 1 Mar 2025, FY2025 statement stating nil units at 0.20/unit. Generation answers the new 422,
the hand-entered row is accepted, the cross-check comes back **empty**, the sale's cost base carries
the reduction (49,800 from 50,000, gain 10,200), and the annual tax report reports the year
complete.

Docs: the generation section's refusal bullet names both cases and points the closed holding at the
hand-entered path (calling it the normal path for the year of a sale); the cross-check's coverage
bullet states the band, its statutory reason, and that both terms are split-aware; the README
feature line and the two web UI descriptions (the cross-check report, and the generate-adjustments
action) follow.

Tests: `reports::amit_adjustment_cross_check::tests::db_a_statement_covering_units_sold_during_the_year_reconciles`
(F-04 exactly — nil units held, rows covering the sold units, no alert),
`db_coverage_beyond_the_units_disposed_of_in_the_year_is_flagged` (the top of the band exactly, then
100 past it with the excess named), `db_a_disposal_before_the_year_does_not_widen_the_coverage_band`,
and the re-worded refusal pinned in
`entities::amit_adjustment_generation::tests::api_each_refusal_returns_422_naming_the_reason`, plus
two `doc_checks` assertions. Full suite 1497 passed / 0 failed.

## Duplicate income rows are silently double-counted (SCENARIOS G-24)
(SCENARIOS.md section G verification pass, 2026-08-16 — the `income` counterpart of the closed
E-03 `duplicate_actions` and F-06 `duplicate_amma_statements` findings.)
- [x] G-24 — two `income` rows for the same listing, holding account and `date_paid`, with identical
  amounts, report twice the dividend income and twice the franking credits. `GET /reports/health`
  says nothing: its duplicate checks cover corporate actions and AMMA statements only
- [x] The cause is the same as those two (a re-submitted form, a re-imported statement) and so is
  the shape of the fix: a **warning, not a constraint** — two dividends from one company on one day
  are legitimate in principle (an ordinary and a special dividend), so the pair must stay enterable
- [x] Open question: the key. (listing, account, `date_paid`) alone flags the legitimate
  ordinary + special pair; adding "identical gross amounts" flags only what is almost certainly a
  double entry
- [x] Tests: a duplicated pair is reported with its ids (as `duplicate_actions` is), rows differing
  in listing/account/date/amount are not, and the web banner names it

**Resolution (2026-08-17): a `reports::health` warning, as for E-03 and F-06 — non-blocking, no
constraint.**

`GET /reports/health` gained `duplicate_income`: one row per (listing, holding account, `date_paid`)
carrying more than one income row of **identical amounts**, newest first, as
`{ listing_id, ticker, holding_account_id, date_paid, currency, gross_amount, income_count,
income_ids }` — the ids ascending, so the surplus row is opened and deleted without a search, and
`gross_amount` (franked + unfranked + foreign source, via `Income::gross_cash_income`) naming the
distribution rather than only its date.

The open question is settled the way the finding's own test list implies: **the amounts are part of
the key**. Keying on (listing, account, date) alone would fire on the legitimate ordinary + special
pair, which differs in what it pays; requiring every money column *and* the currency to match leaves
only what is almost certainly one payment entered twice. Non-money differences (an `ex_date` filled
in on one row only) are ignored — that is how a re-entry differs from the original, not evidence of
a second payment.

Unlike the other two lists this one is **grouped in Rust, not in SQL**: the amounts are TEXT decimal
columns, which SQL would compare as strings, so `70.0` and `70.00` — the same dollars written by two
clients — would slip through. `same_income_entry` compares them as `Decimal`s, over rows the SQL
already narrows to same-(listing, account, date) clusters via an `EXISTS` subquery, so only the
handful of same-day pairs a portfolio has is read into memory. One same-day cluster can hold both a
duplicated pair and a genuine second dividend: the grouping is per amount fingerprint, so the third
row neither joins the pair nor suppresses it.

The web UI's cross-view banner names it — count, gross amount, currency, ticker, date, ids, with
"the dividend and its franking credits are counted once per row; delete the duplicate unless both
are real" — and links to Income beside the existing Jobs, Closing Prices, Corporate Actions and AMMA
Statements links. Verified end to end against a running server seeded with the demo fixture plus a
re-entered dividend: `/reports/health` returns the row (VAS, 2757.30 AUD, 2024-07-01, ids 1 and 999)
and the banner renders it in headless Chrome.

`docs/API.md`'s health section states the field, why it double-counts (the tax summary's dividend
lines, the franking credits, the foreign income and the FITO limit are each summed row by row), why
the amounts are in the key, that they are compared as decimals rather than as stored text, the
recorded decision that this is deliberately *not* a uniqueness constraint, and the way out (delete
the surplus row, `422` while a DRP reinvestment is still linked). The README's monitoring bullet
lists it alongside the duplicated corporate action and AMMA statement.

Tests: `reports::health::tests::duplicated_income_rows_are_reported_with_their_ids` (two listings,
newest first, with the ids and the gross),
`income_differing_in_listing_account_date_or_amount_is_not_a_duplicate` (the key is all four parts —
the ordinary + special pair stays silent),
`amounts_equal_in_value_but_not_in_text_are_still_duplicates`,
`three_identical_income_rows_are_one_row_counting_three`,
`a_duplicated_pair_is_reported_beside_a_genuine_second_dividend`,
`empty_database_reports_nothing_stale` (extended), and `web::tests::health_banner_ui_present`.
Full suite 1545 passed / 0 failed.

## Duplicate interest and expense rows are silently double-counted (SCENARIOS H-01, H-06)
(SCENARIOS.md section H verification pass, 2026-08-17, standing probe 6 — the `interest_income` /
`investment_expenses` counterpart of the closed E-03 `duplicate_actions`, F-06
`duplicate_amma_statements` and G-24 `duplicate_income` findings.)
- [x] Two `interest_income` rows with the same `date_paid`, `amount` `250` and `source`
  "ANZ savings" report `interest_income` `500`; two identical `investment_expenses` rows report
  `deductions_advice_fee` `200`. `GET /reports/health` says nothing — its duplicate checks cover
  corporate actions, AMMA statements and income rows only
- [x] Same cause as the other three (a re-submitted form, a statement keyed twice), and the same
  shape of fix: a **warning, not a constraint** — two interest credits of the same amount on one day
  from different accounts are legitimate, which is why `source` (interest) and
  `expense_type` + `listing_id`/`holding_account_id` + `description` (expenses) belong in the key
  alongside the date and the amount
- [x] Group in Rust, not SQL: the amounts are TEXT decimals SQL would compare as strings — the
  `duplicate_income` pass (G-24) already had to do this, so follow it exactly, banner included
- [x] Tests: a duplicated pair of each kind is reported with its ids, rows differing in any key field
  are not, and the web banner names both new lists

**Implemented 2026-08-17: two more warning lists on `GET /reports/health`, shaped exactly like the
`duplicate_income` pass they follow.**

`duplicate_interest` and `duplicate_expenses` complete the set — every fact table the tax summary
sums row by row now has a duplicate check. Both are **warnings, never constraints**: a payer really
can credit the same amount twice in one day (two equal term deposits maturing together), and two
advice fees of the same amount on one day against different holdings are ordinary entry.

- **The key is the payer identity plus every stored field.** Interest has no listing, so `source`
  (the free-text "ANZ savings account") and `holding_account_id` stand in for one, alongside the
  date, the amount, the currency, `foreign_source` and both withholding figures. For an expense the
  key is the date, the type, the money figures — the `gross_amount` / `deductible_percentage`
  provenance pair included, since two rows agreeing on what is claimed but not on the gross it was
  apportioned from came off different invoices — the currency, the description, and both optional
  attributions. A listing-attributed expense reports its `ticker`, not only the id.
- **Grouped in Rust, as G-24 had to be.** The amounts are TEXT decimals SQL would compare as
  strings, so `same_interest_entry` / `same_expense_entry` compare `Decimal`s (`250.0` and `250.00`
  are the same credit) after a SQL pre-narrowing to rows that share a date with another row. The
  pre-narrowing is on the date alone here — the rest of each key is nullable, and null-safe
  comparison in SQL would buy nothing when same-day rows are a handful.
- **The banner names both**, with its own wording per kind and a link to the screen the surplus row
  is deleted from (Interest Income, Investment Expenses), joining the three existing duplicate
  strips.

Tests: `reports::health::tests::duplicated_interest_rows_are_reported_with_their_ids`,
`interest_differing_in_any_key_field_is_not_a_duplicate` (different source, date, amount, holding
account, and TFN withholding — five near-misses, none flagged),
`interest_amounts_equal_in_value_but_not_in_text_are_still_duplicates` (also three-of-a-kind as one
row counting three), `duplicated_expense_rows_are_reported_with_their_ids` (ticker on the
listing-attributed group, `null` on the portfolio-wide one),
`expenses_differing_in_any_key_field_are_not_duplicates`, the extended
`empty_database_reports_nothing_stale`, and `web::tests::health_banner_ui_present`.

## A deduction's listing attribution never reaches the annual tax report (SCENARIOS H-07)
(SCENARIOS.md section H verification pass, 2026-08-17.)
- [x] H-07 — the correctness side is fine and now pinned
  (`investment_expense::tests::api_expense_survives_a_rename_and_blocks_deleting_its_listing`): a
  rename keeps `listing_id`, and deleting the listing is refused `422` naming the investment expenses
  that draw on it. What is missing is the print surface. `tax_report::DeductionRow` carries
  `listing_id` and no `ticker` — the only listing-bearing row in the report that doesn't — and
  `taxreport.js` renders the deductions table as `date_incurred, expense_type, amount_aud,
  description`, so the attribution is dropped entirely from the document that gets archived as the
  year's PDF
- [x] It matters most in exactly the scenario that raised it: after a rename, demerger, or worthless
  declaration, a bare `listing_id` in the JSON is the only trace of which holding the fee was for,
  and the printed page has not even that
- [x] Fix: carry the ticker on `DeductionRow` the way `DividendIncomeRow`/`ForeignIncomeRow` do (the
  report already loads the listing map), and add the column to the printed table
- [x] Tests: a listing-attributed expense prints its ticker in the annual tax report, a
  portfolio-wide one prints blank, and the served bundle carries the column

**Implemented 2026-08-17: `DeductionRow` carries the ticker and the printed table has the column.**

`ticker: Option<String>` beside the existing `listing_id`, resolved through `IncomeContext`'s
`ticker_as_at` — **as at `date_incurred`**, the same as-at naming every other listing-bearing row in
the document uses, so a fee incurred before a rename prints the ticker the invoice would have named
and one incurred after prints the new one. `None` for a portfolio-wide expense, which prints blank
(`cellText` renders null as an empty cell). `taxreport.js` renders the Deductions table as
`date_incurred, expense_type, ticker, amount_aud, description`, and `docs/API.md`'s annual-tax-report
section documents the field and extends its as-at-naming sentence to cover the date incurred.

The correctness side was already right and already pinned
(`investment_expense::tests::api_expense_survives_a_rename_and_blocks_deleting_its_listing`); this
closes the print surface, which was the only place the attribution was lost outright.

Tests: `reports::tax_report::tests::a_listing_attributed_deduction_prints_its_ticker_as_at_its_own_date`
(a fee either side of a rename prints LAAC then LAR, and a portfolio-wide fee prints no ticker at
all) and the extended `web::tests::annual_tax_report_ui_present`.

## SCENARIOS V-c — a trade entered twice is the one duplication the health report does not look for

Raised driving **V-09** (import a whole portfolio's history in one session and reconcile the final
holdings against a registry statement).

`GET /reports/health` carries a `duplicate_*` check for every other user-entered fact table —
`duplicate_income`, `duplicate_interest`, `duplicate_expenses`, `duplicate_amma_statements`,
`duplicate_ess_statements`, `duplicate_inheritances`, `duplicate_actions`, `duplicate_price_series`
— and none for **trades**, which during a bulk back-entry is the row most likely to be keyed twice
and the most expensive to get wrong.

Measured: two identical Buys of one listing — same date, holding account, price, quantity **and the
same `contract_note_ref: "CN-8891"`** — were both accepted, and health reported nothing. Two
identical income rows entered in the same session were flagged immediately.

A duplicated Buy inflates the holding and the cost base; a duplicated Sell inflates realised gains
and its allocations quietly consume a second parcel. Either is invisible until the holdings are
reconciled against a registry statement, which is the whole of V-09.

Options offered:

1. `duplicate_trades`, keyed the way `duplicate_income` is — listing, holding account, date,
   `trade_type`, `average_price`, `quantity` — over all trade types.
2. The same, but restricted to **user-entered** trades: exclude the rows a derived path creates
   (rollover/transfer/buy-back/rights/ESS-vest/inheritance-linked and reinvest-created DRP), which
   can legitimately repeat.
3. Key on a repeated non-null `contract_note_ref` alone — no false positives at all, but it only
   catches imports that record the broker reference.

**Evan chose option 3** — key on a repeated non-null `contract_note_ref`. No false positives, and
a broker reference repeated across two trades is unambiguous evidence of a double entry.

- [x] Add the `duplicate_trades` health check keyed on `contract_note_ref`, with a test, the
      `docs/API.md` health entry, and the UI health banner wording.
      Keyed on **(listing, trimmed non-null `contract_note_ref`)**: a note can cover a multi-line
      order, so two securities may share one reference legitimately and the listing is part of the
      key; the holding account deliberately is *not*, since the same note re-keyed against the
      wrong account is the worst version of the mistake. Blank/whitespace-only references never
      group (nor NULL ones), and every derived path writes NULL, so those rows fall out by
      construction. Eight tests in `reports::health`, a `doc_checks` pin, the `docs/API.md` entry
      (with the limitation stated: it only catches trades whose entry recorded the reference), the
      README feature line, and the banner row + Trades link.

## SCENARIOS W-c — The tax-return-ready CSV exports carry 28-digit figures under ATO labels

`docs/API.md` calls the two `/export` endpoints "tax-return-ready" and gives each a second header row
mapping its columns to ATO tax-return labels. On **Evan's real database** (a read-only copy of the
2026-08-22 backup) they read:

```
net-capital-gain.csv  FY2026  18A  39592.120176274130543388699381
net-capital-gain.csv  FY2026  18V  0.000000000000000000000000
tax-summary.csv       FY2026       20243.630345624323612748757063
```

18V — the capital loss carried forward, a figure transcribed onto the return — prints as
twenty-four zeros after the point. Every year with a brokerage-bearing disposal in it is affected;
FY2021, FY2023 and FY2024 happen to come out clean, which is why this has gone unnoticed.

The control is the web UI, which is correct: `util.js`'s `COLUMN_KINDS` classifies every one of these
columns as money and `filterableTable` renders them at two decimal places, half away from zero, with
the full value on hover. The rule exists, it is documented at `docs/API.md`'s **Amounts round, rates
don't**, and the CSV export is the one money surface that does not inherit it.

Found by the standing probes while driving **W-07** (sum-of-parts vs total).

Options offered:

- **(a) Round every money column in the two exports to the cent**, half away from zero — the same
  rule and direction the screens use — leaving the JSON API full-precision as documented.
- (b) Whole dollars for the ATO-labelled columns, cents for the rest.
- (c) Round in the report itself so JSON, CSV and screens carry one figure.
- (d) Document as a known limitation.

**Evan chose (a).** The CSV mirrors a screen, so it should read like it; the JSON stays the exact
figure the docs promise.

- [x] Round every money column of `net-capital-gain.csv` and `tax-summary.csv` to the cent (half away
      from zero, the `roundDecimalStr` rule) in `reports::export`'s `csv_response` path, leaving rate
      and quantity columns verbatim and the JSON responses untouched; update `docs/API.md`'s two
      export paragraphs to say so
      — the rounding is a **type**, not a column-name list: `reports::export::Cents(Decimal)`
      serializes to 2 dp, half away from zero, always both places (`0.00`, never `-0.00`, no
      thousands grouping — the separator is the delimiter). A CSV row is a *projection* of the
      report record whose money fields are `Cents`, so which columns round is decided by the
      field's type and nothing duplicates `util.js`'s `COLUMN_KINDS` in Rust — a name list in
      `csv_response` was the alternative and was rejected for exactly that reason (serde hands the
      writer a `Decimal` as a string, indistinguishable from `taxpayer_basis`, so a writer-level
      pass has no way to tell money from text without such a list).
      `NetCapitalGainYearCsv` already existed and gained `Cents` fields; `tax_summary` grew the
      matching `TaxYearSummaryCsv` (39 money columns) rather than exporting the JSON struct
      directly. `tax_year` and `taxpayer_basis` are not money and pass through untouched; the JSON
      responses are unchanged, pinned by a control test on each report over the same facts.
      Verified against the read-only copy of the 2026-08-22 backup: 18A `39592.120176274130543388699381`
      → `39592.12`, 18V `0.000000000000000000000000` → `0.00`, FY2022 18A → `3151.90`, tax-summary
      FY2026 assessable income `20243.630345624323612748757063` → `20243.63`, with both JSON reports
      still answering the full-precision figure. Tests: `reports::export::tests::{a_money_column_rounds_to_the_cent_and_a_plain_decimal_does_not,
      a_half_cent_rounds_away_from_zero_in_both_directions, a_nil_money_figure_is_two_zero_decimals,
      a_whole_or_short_money_figure_is_padded_to_the_cent}`, and on each export
      `api_export_rounds_money_columns_to_the_cent` + its control
      `api_the_json_report_keeps_the_precision_the_export_rounds`
      (`reports::net_capital_gain`, `reports::tax_summary`) plus
      `reports::tax_summary::tests::api_export_rounds_a_half_cent_away_from_zero`;
      `doc_checks::cent_rounded_csv_exports_documented` pins the two export paragraphs and the
      display-rules sentence that had promised full-precision CSV.
      One consequence, deliberate and matching the screens: each column rounds independently, so
      rounded components need not add to a rounded total (here 39344.55 + 247.57 = 39592.12 does,
      but that is arithmetic, not a guarantee) — the same behaviour every table on screen has.

## SCENARIOS W-d — The Annual Tax Report's printed columns do not add up

The Annual Tax Report is the one surface built to be printed and archived (`custom: 'tax-report'`,
its own `@media print` stylesheet, A4 landscape). Its parcel rows and its subtotals are each rounded
to the cent independently, so the column does not add up on the page. An entirely ordinary
three-parcel BHP disposal — `$9.95` brokerage plus `99.5c` GST on each buy, so each parcel's cost base
lands on a half-cent — prints:

```
parcel discount amounts   63.55 + 527.90 + 1060.73 = 1652.18
printed group subtotal                               1652.17
```

The subtotal is the exact sum rounded; the rows are each rounded, three of them upward. The same
disposal's `cost_base` column is a cent out for the same reason. At four decimal places every column
reconciles, which is the control — the arithmetic is right and the presentation is what disagrees.

Found by driving **W-07** directly, and confirmed against the printed document rather than only the
JSON.

Options offered:

- **(a) Total the rounded rows, in the report** — round each parcel figure to the cent in
  `reports::tax_report` and make each subtotal and grand total the sum of those rounded figures, so
  the API and the printed page agree and a reader can add the column up.
- (b) Do the same in `taxreport.js` only, leaving the API exact (so the two then differ).
- (c) Print rows at four decimal places.
- (d) Document as a known limitation.

**Evan chose (a).** The document's job is to be checked by hand; a column that does not add up fails
at exactly that.

- [x] Round each disposal-schedule parcel figure to the cent in `reports::tax_report` and sum the
      rounded values into every subtotal and grand total, with a test asserting each column's rows
      sum exactly to the total it sits under; note the convention in `docs/API.md`'s Annual tax
      report section
      — the rounding happens **once, at the row**: `DisposalParcelRow::round_money_to_cents` runs as
      each row is built, so `DisposalTotals::add` can only ever sum figures that are already
      rounded and a subtotal is the sum of the rows printed above it *by construction*, not by a
      second pass someone could forget. The rule itself is now `infra::decimal::to_cents` (to the
      cent, half away from zero, a figure that rounds to nil normalised to a positive zero) and
      W-c's `reports::export::Cents` delegates to it: that type is a *serialisation* wrapper whose
      `Display` renders `{:.2}`, and here the rounded value has to be **summed**, so the two now
      share the rounding and differ only in the rendering — one rule provably, rather than two
      that agree today. Money, and so rounded: `initial_cost_base_aud`, `adjusted_cost_base_aud`,
      `proceeds_aud`, `gain_loss_aud`, `cgt_discount_amount_aud`, `gain_after_discount_aud`, and
      each itemised adjustment's `amount` (the document prints those under the parcel too). Left
      verbatim: the two `*_per_unit_aud` figures and an adjustment's `per_unit` — a derived
      per-unit figure shows at 4+ dp by the documented display rule, never cent-rounded —
      `buy_price`/`sale_price`, `units`, `days_held`, the two FX rates, and
      `buy_brokerage`/`buy_gst_on_brokerage`, the contract note's own native-currency figures,
      transcribed for hand-checking against it (99.5c of GST on $9.95 of brokerage is genuinely
      sub-cent) and totalled nowhere. Nothing downstream reads these rows — the report computes
      nothing new — so no tax figure moved: `realised_gains` and `net_capital_gain` still answer
      the exact decimal, and the tax-report/realised-gains reconciliation test still passes.
      Measured on the three-parcel BHP disposal: the discount and discounted-gain columns now
      subtotal **1652.18** over rows of 63.55 + 527.90 + 1060.73 (printed 1652.17), and the
      cost-base column **27453.44** over 4991.52 + 9227.54 + 13234.38 (printed 27453.43); proceeds
      and gain/loss happened to reconcile already and are unchanged (30757.77, 3304.34).
      Tests: `reports::tax_report::tests::{api_a_disposal_columns_rows_add_up_to_its_printed_subtotal,
      api_every_disposal_money_column_totals_the_rounded_rows_beneath_it,
      api_the_per_unit_and_as_entered_disposal_columns_are_not_cent_rounded}` — the middle one
      finds the money columns *by name* (`*_aud` less the per-unit pair) across a three-group
      document (an AUD disposal, an AMIT parcel carrying itemised adjustments, a USD parcel whose
      every figure is an FX conversion), so a newly added money column is covered without being
      listed, and asserts its total↔parcel column pairing covers the whole of `DisposalTotals`, so
      a newly added *total* fails until it is reconciled too. Plus
      `infra::decimal::tests::to_cents_rounds_half_away_from_zero_and_keeps_the_cent_scale` and
      `doc_checks::cent_rounded_tax_report_disposals_documented`; all three W-d tests were
      confirmed to fail with the rounding call removed. `taxreport.js` needed no change: it prints
      the server's subtotals and re-derives nothing client-side, and `numericDisplay`'s money
      rounding is idempotent on a figure already at the cent (its hover tooltip simply stops
      appearing, having nothing left to show).

Two corrections to this section's own write-up, found by re-deriving it. The cost-base rows are
4991.52 + 9227.54 + **13234.38** (the third parcel is 333 units at 39.71, not 11726.38) — the total
27453.44 was right. And the control is narrower than stated: at four decimal places the *cost base*
column reconciles, but the discount column does not (63.5468 + 527.8979 + 1060.7254 = 1652.1701
against a subtotal of 1652.1700), because halving three exact-arithmetic gains lands on figures no
display precision reconciles. The true control is that the underlying arithmetic is exact and it is
any *display* rounding of the rows that disagrees with the rounded exact total — which is why the
fix has to be to total the rounded rows rather than to print more places.

Three residues deliberately left, each a decision rather than an oversight, and each Evan's to take
as its own section:

1. **A row's own arithmetic can still be a cent out.** The schedule prints proceeds, cost base and
   gain/loss on one line; rounded independently, the second BHP parcel prints 10283.33 − 9227.54
   beside a gain of 1055.80. Deriving the gain from the rounded components would fix the row *and*
   keep the columns adding up (Σ of derived gains = Σ proceeds − Σ cost base), at the price of a
   printed gain that is not the rounded gain — a figure the chosen option (a) does not authorise.
   This is unchanged by W-d: the printed page has shown those same three rounded numbers all along,
   since the UI rounds every money cell.
2. **`income` vs the overall tax summary is the same shape, one level up.** The income tables print
   per-record AUD figures whose totals appear in the tax-summary section, and `docs/API.md`
   currently promises "Every AUD figure here sums to exactly the matching tax summary line" —
   which is true only at full precision. Rounding the income rows would break that documented
   guarantee unless the summary line were rounded too, and that line is `tax_summary`'s, shared
   with its own screen and CSV. Left alone deliberately.
3. **`cgt_summary` likewise**: it is `net_capital_gain::CgtSummaryYear`, printed as a worksheet
   whose lines subtract from one another, and rounding it here would fork the figure from the
   report that owns it. See the note under W-c for the same fault in that report's CSV.

## SCENARIOS W-f — The tax-return CSV's printed working does not reconcile to the figure it works to

Surfaced by [W-c](#scenarios-w-c--the-tax-return-ready-csv-exports-carry-28-digit-figures-under-ato-labels) and confirmed while building [W-d](#scenarios-w-d--the-annual-tax-reports-printed-columns-do-not-add-up): now that each
money column of `net-capital-gain.csv` rounds to the cent independently, the columns that are
*arithmetically related to each other* need not agree. Reproduced at the export endpoint on an
entirely ordinary single-parcel disposal — 100 units bought 2022-01-05 at $10, sold 2024-03-15 at
$11.0001, no brokerage:

```
discount_eligible_gains  100.01   (label 18H component)
cgt_discount              50.01   (label 18 working)
net_capital_gain          50.01   (label 18A)
```

The printed working reads **100.01 − 50.01 = 50.00**, beside an 18A of **50.01**. The exact figures
are 100.01, 50.005 and 50.005, and the 50% discount is the same halving mechanism that produced W-d.
**Any year whose discount-eligible net gain is an odd number of cents does this.** Evan's own data
reconciles (`39344.55 + 247.57 = 39592.12`) by luck rather than by construction.

This is the CSV twin of W-d, one level up: W-d made a *column of rows* add to its total; this is a
*row of columns* that form a worksheet. `tax-summary.csv` carries the same risk wherever one column
is a sum of others. Note the divergence is **not** new breakage — the same figures rounded the same
way on screen before W-c; what W-c changed is that the CSV now shows the rounded form too, which is
what makes the inconsistency legible rather than hidden behind 28 digits.

Two things bound it before any fix is aimed:

- The **underlying tax figure is right** either way; this is a presentation inconsistency in a
  document meant to be transcribed, not a wrong assessable amount.
- It lives in `reports::net_capital_gain`'s year record, which the JSON report, the CSV export **and**
  the Annual Tax Report's `cgt_summary` section all share — W-d deliberately left `cgt_summary`
  alone for exactly this reason, so whatever is decided here should settle all three at once rather
  than forking the figure.

Options offered:

- **(a) Derive the worksheet after rounding** — round the input columns to the cent, then compute the
  dependent ones from the rounded values in the shared year record, so the JSON, the CSV and the
  annual report's `cgt_summary` all reconcile and the printed working adds up. Moves the reported
  figure by up to a cent.
- (b) Leave the figures and document the possible cent of disagreement.
- (c) Round half-to-even, which splits the `.005` cases evenly rather than always up.
- (d) Defer to a later section.

**Evan chose (a).** A worksheet whose working does not reach its own result fails at the one job the
document has. (c) was rejected on its own terms: it lowers the frequency without removing the
divergence, and it would fork the rounding rule from the half-away-from-zero the screens, the CSV
exports and `infra::decimal::to_cents` now all share.

- [x] Derive the dependent worksheet columns from the cent-rounded inputs in the shared
      net-capital-gain year record, so the JSON, the CSV export and the annual tax report's
      `cgt_summary` reconcile; carry the same treatment to `tax-summary.csv` wherever one column is a
      sum of others
      — the rounding moved **into `net_years`**, the shared year record, so all three surfaces read
      one worksheet. The dependency graph, derived from the code rather than assumed: four
      **inputs** (`discount_eligible_gains`, `other_gains`, `capital_losses`, and
      `capital_loss_brought_forward` — which is the *previous* row's `capital_loss_carried_forward`,
      so rounding it only bites on the chain's seed, the entered `cgt_settings` opening loss) plus
      three informational ones (`cgt_event_e10_gain`/`_g1_`/`_c2_`, in no printed working but money,
      so rounded too), and five **derived** (`net_other_gain`, `net_discount_eligible_gain`,
      `capital_loss_carried_forward`, `cgt_discount`, `net_capital_gain`). Every input goes through
      `infra::decimal::to_cents`; the loss netting is `+`/`−`/`min` over cent figures and so cannot
      leave the cent; **only the halving rounds again**, and it is the *discount* that rounds — the
      worksheet's own "less CGT concession amount @ 50%" line — with
      `net_capital_gain = net_other + (net_discount − cgt_discount)`, i.e. what the worksheet leaves
      after the line printed above it rather than a second halving. That is the direction that makes
      the printed working reconcile, and (the discount rounding half away from zero) lands the
      assessable figure the taxpayer-favourable way: the finding's disposal now reads
      **100.01 − 50.01 = 50.00** with an 18A of **50.00**. `CgtSummaryYear` follows for free bar one
      figure — `amma_discount_gains_grossed_up` is rounded and `long_term_gains` **derived** from it
      by subtraction (`to_cents` is monotonic, so the remainder can never go negative), so the
      printed worksheet's two gain lines add to `discount_eligible_gains` exactly.
      **What `docs/ato/` says about worksheet rounding: nothing.** Question 18's own step order
      (`capital-gains-question-18.md`, and a fresh fetch of the live page today — 807 lines, zero
      occurrences of "cents", "round" or "whole dollar") is silent, as are `cgt-how-to-calculate.md`
      and `cgt-discount.md`. The only rounding rules anywhere in the mirror are somebody else's: the
      trustee-level AMIT rounding adjustment surplus/deficit
      (`amit-calculating-trust-components.md`, Div 276 — a trust's problem, not a member's), the
      indexation factor's three decimals, and the per-label "show cents" notes
      (`tax-return-labels-2026.md`, `amma-statement-guidance-notes.md`) — which cover 10M/11V/13P-S/
      13A/13B/20O and **not** 18A/18H/18V, so a cent never reaches the lodged return at question 18
      at all. The divergence was only ever visible to a human checking the worksheet, which is
      exactly who this document is for. Silence means the direction was chosen on the project's own
      rule (one `to_cents`, half away from zero, shared with the screens) rather than on an ATO
      instruction.
      **`tax-summary.csv` does diverge**, measured before the fix on two income lines and two
      expenses landing on half cents: `gross_assessable_investment_income` printed `70.01` over
      lines of `60.01 + 10.01`, and `deductions_total` printed `20.01` over destination lines of
      `10.01 + 10.01`. Its three total columns are now the sum of the cent-rounded lines beside
      them. Its **income lines are deliberately left exact**, which is the one place the treatment
      differs from the net-capital-gain worksheet and the reason is a documented cross-report
      contract: `docs/API.md` promises the annual tax report's per-record income rows "sum to
      exactly the matching tax summary line", and those rows are W-d's residue 2, left for a section
      of their own — rounding the line would have broken that promise silently (the existing test
      uses whole dollars and would still pass). Nothing is derived from an income line but the
      totals, so rounding one level up costs nothing. The **deductions** could not be handled that
      way: they are cut two ways (by kind and by destination) and both cuts are printed beside one
      `deductions_total`, so each expense is instead converted and rounded **at its own row**
      (W-d's rule) and both cuts sum to the total by construction; `tax_report`'s deduction row
      `amount_aud` is rounded with it, so that drilldown still sums exactly.
      **Consumers, each confirmed:** the JSON report and CSV export (one record now, pinned by a
      test each way); the annual tax report's `cgt_summary` (new test asserting it agrees with both
      *and* that its own printed lines subtract to one another) and its `tax_summary` line section
      (prints the record verbatim, so it inherits the fix); `db_cgt_years`, which reads only
      `tax_year`; the pre-sale what-if, whose two scenario rows run through the same `net_years`
      (its `hypothetical` totals stay the realised-gains figures, exact — it is a dry-run, not a
      worksheet); `entities::worthless` and `ato_examples`, which assert whole-dollar figures and
      are unmoved. **On Evan's real database** (a read-only copy of the 2026-08-22 backup, since
      deleted) two years move by exactly one cent and both now reconcile: FY2026 18A
      `39592.12` → `39592.11` (78689.09 − 39344.55 = 39344.54, + 247.57) and FY2025 `5076.87` →
      `5076.86`; FY2021–FY2024 are unchanged, and every `tax-summary.csv` total already reconciled
      (no investment expenses recorded). **A correction to this section's own write-up:** Evan's
      FY2026 did *not* reconcile "by luck" — `39344.55 + 247.57 = 39592.12` checks the *addition*
      form, which is the old code's own formula and so could never fail; read as the worksheet
      prints it (18H net *less* the concession) it was a cent out, exactly like the reproduction.
      Tests: `reports::net_capital_gain::tests::{api_export_the_printed_working_reaches_the_figure_it_works_to,
      api_every_derived_column_is_the_arithmetic_of_the_cent_rounded_inputs,
      api_the_annual_tax_reports_cgt_summary_agrees_with_the_json_and_the_csv,
      api_a_year_already_exact_at_the_cent_is_unchanged (the control),
      api_the_json_report_carries_the_same_cent_figures_as_the_export (W-c's control, rewritten:
      W-f is what reversed it)}` — the generic one requires **every** field of the record to be
      classified as an input, a derived column, or not money, so a newly added column fails until it
      is placed, and re-derives the netting, the halving and the year-to-year loss chain from the
      reported figures; `reports::tax_summary::tests::{api_export_a_total_column_is_the_sum_of_the_columns_it_totals,
      db_each_total_column_totals_the_columns_beside_it,
      db_a_year_already_exact_at_the_cent_is_unchanged (the control)}`;
      `reports::tax_report::tests::income_sections_still_sum_to_the_summary_line_on_half_cent_figures`
      (the drilldown promise, on the facts that would break it). All four W-f tests and all three
      tax-summary ones were confirmed to fail with the rounding removed. Docs: `docs/API.md`'s
      **Amounts round, rates don't**, Net capital gain (a new *worksheet is kept at the cent*
      paragraph, and the export paragraph's "the JSON report above is unaffected" **withdrawn** —
      W-c's promise, which W-f deliberately reverses), Tax summary (a new *a total column is the sum
      of the columns printed beside it* paragraph), and the annual tax report's `cgt_summary` and
      income bullets; pinned by `doc_checks::worksheet_derived_columns_documented`.

## The parcel optimiser's candidates are costed as *held*, not as *disposed of*

Raised while sweeping `reports::parcel_optimiser.rs`'s pro-rate onto `mul_div` (the section archived
in [`DONE/tax-domain.md`](tax-domain.md) flagged the line as `domain::cost_base`'s pro-rate
re-implemented locally and asked whether the local copy had drifted).

The pro-rate itself has **not** drifted: `disposal_figures` starts from `remaining_cost_base`, which
`reports::open_parcels` already obtained from the shared pipeline, and scaling it linearly by
`units ÷ remaining_quantity` agrees with `domain::cost_base::adjusted_cost_base` to the digit —
measured against `reports::realised_gains` for the same disposal actually recorded, across a plain
partial sale, a return of capital, a whole-parcel pick, and an AMMA statement whose year end
precedes the sale. (A consolidation into a non-terminating unit basis differs by 2e-27, which is the
second pro-rate rounding once more at 28 digits, not a disagreement about the rule.)

What **has** drifted is one step upstream of that line, and it is the same class of bug — two
callers of one domain calculation disagreeing. `parcel_optimiser::db_candidate_parcels_on` reads its
candidates through `domain::open_parcels::load`, which costs them `Held::AsAt(as_of)`; the recorded
Sell is costed `Held::DisposedOn(sale_date)`. The two differ in exactly one respect —
`Held::AsAt` reports no disposal date, so `AmitReductionEvent::reduction_for_units` always takes its
*still-held* branch — and that changes the figure whenever an AMMA statement's tax year end falls
**after** the contemplated sale date and its adjustment row covers more than the units still held at
that year end. The optimiser then applies no reduction at all, while the same disposal, once
recorded, spills the statement onto the disposed units per s 104-107B / LCR 2015/11 (the adjustment
is made just before the CGT event).

Measured: a 100-unit parcel at $13.3166…/unit, an AMMA for the year ended 2026-06-30 with a −$0.13
per-unit cost-base adjustment covering the whole parcel, and 40 units disposed of on 2026-03-02.
`POST /portfolio/parcel-optimiser` costs the 40 units at **A$532.67**; the same 40 units recorded as
a Sell are costed **A$537.87** by `/portfolio/realised-gains` — A$5.20 apart, the statement's whole
reduction on those units. `POST /reports/net-capital-gain/what-if` shares the same reader and so the
same figure. Narrow (it needs a contemplated sale dated before a recorded statement's year end) but
live, and silent: nothing marks the estimate as resting on a different rule from the report it is
meant to predict.

Reproduced independently on a second fixture before this was accepted: a 100-unit VDHG parcel at
$60, an AMMA for the year ended 2026-06-30 carrying a **$1.30** per-unit adjustment over the whole
parcel, 40 units contemplated on 2026-03-02. The optimiser costs them **A$2,400.00**; the same 40
units recorded as a Sell are costed **A$2,348.00** — A$52.00, again exactly the statement's whole
reduction on the disposed units (40 × $1.30). The gap therefore **scales with the per-unit
adjustment and the units sold**, and is not inherently small: a Vanguard AMMA's per-unit cost-base
adjustment is routinely in this range, so a four-figure holding puts it in the hundreds of dollars.

Worth noting *which* way it errs. The optimiser reports the **higher** cost base, so it under-states
the gain the sale will actually realise — and because it ranks its four strategies on those gains, a
statement covering one candidate parcel and not another can reorder the ranking, not merely shift
every row by a constant. The output is advice about which parcel to sell, so a wrong ordering is the
failure that matters, not the wrong figure beside it.

This is a fresh instance of the pattern SCENARIOS section O already named — *diff a decision-support
endpoint against the write path it rehearses* — which is how it should be tested once fixed: not by
asserting a figure, but by asserting the optimiser and the recorded Sell agree over a matrix of
cost-base events.

- [x] Decide whether the optimiser and the pre-sale what-if should cost their candidates as
      *disposed of on the contemplated date* rather than as *held at it* — a `Held` the loader is
      told rather than infers — and either make the two agree or say in `docs/API.md` where they
      deliberately do not, with a test pinning whichever answer is chosen
      — **made to agree**, and the fix is one step larger than the finding's own diagnosis. Two
      corrections to the write-up above. First, the **mechanism**: the reproduction's A$52.00 gap
      is not `Held::AsAt` taking `reduction_for_units`' still-held branch — it is
      `open_parcels::load` passing its as-of date to
      `amit_adjustment::db_cost_base_reduction_events`, whose `tax_year_end_date <= ?` filter drops
      a statement for a year that has not ended, so the optimiser applied no AMIT event *at all*
      (6000 → 2400 for 40 of 100 units). `realised_gains` reads its statements unbounded, which is
      why the recorded Sell saw it. Second, and the reason the minimal shape the finding proposed
      would not have worked: passing `Held::DisposedOn(sale_date)` while leaving
      `AmitReductionEvent::disposed_by_year_end` as the *recorded* allocations read it takes the
      spill branch with `sold = 0` and returns **zero** — the same wrong answer, arrived at
      differently. The contemplated units have to join `disposed_by_year_end` themselves, and that
      is what makes the pipeline **non-linear** in the disposed units (the finding's linearity
      question, answered): with a partly-covering AMIT row the reduction reaching `u` units is
      `per_unit × (covered − held)⁺ × u ÷ (D + u)`, whose denominator moves with `u`, so
      `disposal_figures`' pro-rate off the whole parcel could not be kept. Measured: with the rest
      of the fix in place but the pro-rate restored, a 100-unit parcel whose FY2026 row covers the
      70 open at that year end estimates A$2,363.60 for a 40-unit pick against the A$2,370.2857…
      the Sell realises.
      Built: `domain/contemplated_disposal.rs` — the shared "cost a disposal that is not recorded
      yet" calculation (`Costing::load` reads the AMIT/ROC/split events and `FxRates` on the
      caller's own connection; `adjusted_cost_base_aud` re-bases the units to the as-acquired
      basis, adds them to `disposed_by_year_end` for every statement whose year end the sale falls
      on or before, and runs `domain::cost_base::adjusted_cost_base` under
      `Held::DisposedOn(sale_date)`), carrying the s 104-107B / LCR 2015/11 citation for why a
      statement for the year the sale falls inside reaches the sold units. `parcel_optimiser`'s
      `db_candidate_parcels`/`db_candidate_parcels_on` now answer a `Candidates` — the candidate
      set still from `domain::open_parcels::load` (unchanged, so no other caller's figures moved:
      the loader, `reports::open_parcels`, portfolio, unrealised gains, rollover consistency and
      AMIT-adjustment generation are untouched by the diff), the cost bases from `Costing`, and the
      sale date held once so `disposal_figures` cannot be handed a different one. `disposal_figures`
      costs each pick at its own unit count instead of pro-rating. The what-if follows through the
      same reader, inside its own single read transaction.
      Tests (`reports::parcel_optimiser::tests`) are the section-O pattern the finding asked for —
      *diff a decision-support endpoint against the write path it rehearses* — a harness that asks
      the optimiser, records **exactly** the Sell it described, and requires
      `/portfolio/realised-gains` to agree per allocation and in total rather than asserting a
      figure: `agreement_on_a_plain_partial_sale`, `agreement_on_a_whole_parcel_pick`,
      `agreement_when_the_amma_year_end_falls_after_the_sale` (the finding),
      `agreement_when_the_amma_year_end_falls_before_the_sale` (the control that already worked),
      `agreement_when_the_amit_row_covers_less_than_the_parcel`,
      `agreement_when_a_partly_covering_amit_row_is_taken_in_full`,
      `agreement_on_a_return_of_capital`,
      `agreement_across_a_split_between_acquisition_and_the_sale`,
      `agreement_across_two_parcels_with_every_cost_base_event`; plus
      `the_amma_reduction_reaches_the_estimate_of_a_sale_inside_its_year` (the reproduction's own
      A$2,348.00), `an_amma_inside_the_sale_year_reorders_the_strategies` (the failure that
      matters: min-gain now advises a *different parcel*), and
      `the_what_if_costs_the_same_disposal_the_same_way`; and
      `domain::contemplated_disposal::tests::{a_sale_inside_the_statement_year_joins_its_disposed_units,
      a_sale_after_the_statement_year_leaves_it_alone}`. Confirmed failing with the fix reverted:
      restoring the held-at-the-date costing fails 8 of the 12, and restoring the pro-rate alone
      fails `agreement_when_the_amit_row_covers_less_than_the_parcel`. Docs: `docs/API.md`'s
      Parcel-selection optimiser and Pre-sale what-if sections now say the candidates are costed
      **as disposed of on the contemplated date**, each allocation at its own unit count, and why
      that carries a statement whose year end falls after the sale onto the sold units.


## SCENARIOS X-a — a fact write that lands while a snapshot is being generated is lost, and the snapshot is stored as fresh

`reports::snapshot::generate` reads everything it stores **outside** the transaction it stores it in.
`aud_prices_for`, `portfolio::db_holdings`, `unrealised_gains::db_unrealised_gains` and
`performance::db_performance` each open (and close) their own read transaction against the pool;
only afterwards does `write_tx` open, and its `INSERT … ON CONFLICT … DO UPDATE` writes `stale = 0`.
So a fact write that commits **between** the reads and that insert is neither in the stored figures
nor reflected in the stored flag: the schema's staleness triggers fire against a row that does not
exist yet (or against the row about to be overwritten), and the insert then clears the flag they set.
The snapshot is silently a snapshot of a state that no longer exists, and — because nothing but the
`stale` flag ever asks for a regeneration — it stays that way forever. The daily `report-snapshot`
job, `POST /report_snapshots/generate`, `regenerate_all` and `regenerate_provisional` all go through
this one function.

**Reproduction** (throwaway database, 6,000 parcels on the exchanged listing so generation takes
1.65 s; every price for 2025-06-30 entered at `10`):

1. `POST /report_snapshots/generate {"date":"2025-06-30"}`
2. 500 ms later, while it is still computing, correct one listing's price:
   `PUT /closing_prices/6/2025-06-30 {"price":"25", …}` → `204` in 2 ms.
3. The snapshot lands at `stale: false`, holding `current_price: "10"` and
   `market_value: "6000000"` for that listing — while the stored closing price for that very day is
   `25`, i.e. a market value of `15000000`. The archived valuation is out by **A$9,000,000** and
   nothing will ever say so.

**Bounded by its controls, both of which are correct.** The same correction applied *entirely
before* generation gives `current_price: "25"` / `15000000`; applied *entirely after* it, the three
snapshots come back `stale: true` and are regenerated by the daily job. Only the interleave is
wrong, which is what identifies the read/write split as the mechanism rather than the trigger set.
**It is not price-specific**: repeating the race with an ordinary `PUT /trades/:id` (a Buy dated
before the snapshot date) leaves the snapshot's quantity at the pre-trade `600000` and, again,
`stale: false` — *any* fact write landing in that window is lost the same way. And it needs no
human: the scheduler's own `price-import` and `report-snapshot` jobs write and read the same tables,
and `rba_fx_rate::true_up_provisional_snapshots` regenerates from inside another job's run.

**Evan's real data is clean** (checked on a copy of `share-tracker-2026-08-22-220812.db`):
regenerating all **2,182** stored snapshot dates reproduced every stored `rows_json` byte for byte,
so the race has not yet corrupted anything live. The window there is ~41 ms per date rather than
1.65 s, which is why.

**The other seven section X scenarios came back correct** — see
[Section X findings](../SCENARIOS.md#section-x-findings).

**Options considered** (Evan asked for the pass to pick and proceed rather than stop per finding):

(a) **Generate inside the transaction that stores it** — open `write_tx` first, take every read on
    that connection (`_on(&mut conn)` variants beside the existing `portfolio::db_holdings_on`),
    insert, commit. The inputs and the `stale = 0` claim then come from one serialised point in
    time, which is the rule every other write in the tree already follows. Cost: generation holds
    SQLite's write lock for its duration (~41 ms per date on the real database, 1.65 s on a
    10,000-parcel synthetic), so a concurrent write waits rather than failing — the busy timeout
    covers it, and nothing in `generate` touches the network, so the lock is never held on I/O.
(b) **Detect the interleave and store `stale = 1`** — capture a change marker before the reads,
    re-read it inside the write transaction, and flag the row rather than clearing the flag when it
    moved. Cheaper on lock hold time, but needs a marker the schema does not have
    (`PRAGMA data_version` is per-connection, and the pool hands out a different connection for each
    read), and it *stores a figure known to be wrong* and relies on a later regeneration.
(c) **Leave it, document it as a known limitation.** Rejected: the wrong figure is indistinguishable
    from a right one on the Portfolio Overview graph and in the Snapshots screen, and the archive is
    the only record of a past day's position.

**Chosen: (a).** It removes the window rather than reporting it, it is the convention the rest of
the tree already states (`infra::db::write_tx`'s doc comment, and every entity's `db_upsert`), and
the `_on` split it needs is a pattern the reports already have.

- [x] Generate a report snapshot inside the write transaction that stores it, so a fact write cannot
      land between its inputs and its `stale = 0`
      — `reports::snapshot::generate` now opens `infra::db::write_tx` as its **first** statement and
      takes every input read on that transaction: `aud_prices_for` → `valuation::stored_valuations_on`,
      `portfolio::db_holdings_on` (already existed), `unrealised_gains::db_unrealised_gains_on`,
      `performance::db_performance_on`. The figures and the `stale = 0` they are stored with now come
      from one serialised state, so the write lock is what closes the window rather than a marker
      that detects it. **The mechanism was re-derived before the fix, and the write-up held up in
      full**: the reads each opened and closed their own transaction against the pool, the
      `INSERT … ON CONFLICT … DO UPDATE` sets `stale = 0` on both arms, and both trigger cases behave
      as described (a first generation has no row for the staleness triggers to mark; a regeneration
      has one, marked and then cleared by the same insert).
      **The `_on` split, following `portfolio::db_holdings_on`** — each pool-taking function is kept
      and delegates to its `_on` twin, so the two can never diverge: `reports/valuation.rs`
      (`held_markets_on`, `stored_valuations_on`), `reports/unrealised_gains.rs`
      (`db_unrealised_gains_on`), `reports/performance.rs` (`accumulate_on`, `db_performance_on` —
      `db_performance` now owns the read transaction `accumulate` used to open),
      `entities/closing_price.rs` (`HeldTimeline::load_on`, `db_held_listing_ids_on`, and `db_get_one`
      / `db_latest_ok_price_on_or_before` made executor-generic in the shape `listing::db_get` and
      `FxRates::load` already use, rather than grown a second copy). `load_market_on` was already
      there. **Nothing inside the transaction touches the network**, checked call by call down
      `stored_valuations_on`: it reads `closing_prices`, `listings`, the rename chain, the holiday
      calendar and `rba_fx_rates`, and `Market::latest_complete_trading_day` is pure arithmetic over
      the `now` passed in — snapshot generation values from **stored** prices only, so the lock is
      never held across I/O. `DEFERRED_BEGIN_ALLOWED` is unchanged and still true: the `_on` variants
      begin nothing, and `valuation.rs` reaches for `pool.acquire()` rather than a transaction, so it
      stays off the list.
      **Verified at the HTTP surface, throwaway database, 6,000 parcels (generation ≈ 0.6–1.4 s), the
      finding's own reproduction**: `POST /report_snapshots/generate {"date":"2025-06-30"}` with a
      `PUT /closing_prices/1/2025-06-30 {"price":"25"}` fired 500 ms in. *Before*: the correction
      returned `204` in **1 ms** and the snapshot landed `stale: false` holding `current_price: "10"`
      / `market_value: "6000000"` against a stored price of `25` — A$9,000,000 of archived valuation
      with nothing left to ask for a regeneration. *After*: the same correction returns `204` in
      **81 ms** (it waits for the run) and all three snapshots land **`stale: true`** — the run's own
      figures, correctly flagged, and a following `generate` stores `current_price: "25"` /
      `market_value: "15000000"` fresh. The non-price half of the finding behaves the same way: a
      `PUT /trades/9001` (a Buy dated before the snapshot date) fired 500 ms into a run returns `204`
      in 740 ms and leaves the snapshot `stale: true`, where it used to leave it fresh at the
      pre-trade quantity. **Both controls still hold, unchanged by the fix**: the correction applied
      *entirely before* a run gives `current_price: "25"` / `15000000` fresh; applied *entirely
      after*, all three come back `stale: true`.
      **Tests** (`src/reports/snapshot.rs`):
      `reports::snapshot::tests::a_price_written_during_generation_never_leaves_a_fresh_superseded_snapshot`
      is the invariant — six rounds (the first a first generation, the rest regenerations) each fire
      a corrected price at a run started on another task, and assert the stored snapshot is **either**
      valued at that price **or** flagged stale, plus that its market value still equals its own
      stored price × units. It holds for every ordering, so it cannot flake on the fixed code; it
      fails on round 0 of the old code (3/3 runs).
      `reports::snapshot::tests::generation_reads_only_after_it_holds_the_write_lock` is the same
      guarantee with the race removed: another connection holds `BEGIN IMMEDIATE` with a corrected
      price uncommitted, the run must make no progress, and after the commit it must value at the new
      price — deterministic, and it fails on the old code (which reads before it blocks and stores
      the superseded figure). Both wait on conditions with deadlines, never on yield counts.
      **Both tests build a file-backed pool (`race_pool`, `tempfile` + `infra::db::init`) rather than
      `test_support::test_pool`**, and this is load-bearing rather than incidental: the in-memory pool
      *does* hand out several connections that share one database, but it is **shared-cache**, where a
      reader on a second connection blocks on an open writer — the read/write interleave cannot arise
      at all there, and both tests passed against the *unfixed* code on it. Under WAL (what `main`
      opens) a reader sees the snapshot it began with while another connection commits past it, which
      is the real behaviour. The helper says so, so the next reader does not "simplify" it back.
      **Docs**: `docs/API.md`'s Report snapshots section states the guarantee (reads inside the
      storing transaction, a concurrent fact write waits and then stales, no network call under the
      lock, and the cost — a concurrent write waits tens of ms per date), and README's snapshot
      feature bullet gains the same clause. No schema change, so `docs/SCHEMA.md` is untouched; the
      requirement is code-tested, so no `doc_checks.rs` entry.

## SCENARIOS Z-f — a trust distribution is reported under the dividends-from-companies label

- [x] Report a non-AMIT trust distribution's components at question 13, not inside `dividends_assessable`.

Found driving **Z-11** (the full financial year, reconciled), whose whole job is to tie every figure
in the [annual tax report](../docs/API.md#annual-tax-report) back to a hand-computed return. Thirteen of
the fourteen labels reconciled exactly — 11U/13Q, 10L, 10M, 20E (both lines), 20O, Item 12, D7, D8,
18H, 18V and 18A (the one-cent difference on 18A is W-f's deliberate rounding of the concession
half away from zero, not an error). One did not:

| | franked | unfranked | credits |
| --- | ---: | ---: | ---: |
| a **company** dividend row | 11T | 11S | 11U |
| an **AMMA** statement's components | `amma_franked_dividends` → **13C** | `amma_dividends_unfranked` → **13U** | 13Q |
| a **non-AMIT trust** income row (a managed fund, an ordinary unit trust) | rolled into `dividends_assessable` → **11S + 11T** | same | 13Q ✔ |

A managed-fund distribution of A$900 franked + A$600 unfranked was reported inside
`dividends_assessable` = 7,550.00 under the label **`11S + 11T`**. Those amounts belong at question
13 (*Partnerships and trusts*), not question 11 (*Dividends*). A taxpayer transcribing the summary
puts A$1,500 of trust income under dividends from companies, and the year's question-13 income is
understated by the same amount — there is no line for it at all.

**The credits line already knows better.** `franking_credits` is labelled **`11U / 13Q`**, explicitly
covering both routes; and the AMMA path carries proper `13C`/`13U` lines. So the two correct
destinations are already in the report — the ordinary trust row is the one case that falls through
to the dividends line. The [Annual Tax Report](../docs/API.md#annual-tax-report) is not confused about
what the row *is*: it prints a separate `income.trust_income` drilldown table beside `income.dividends`.
Only the labelled summary line collapses them.

**The documentation states the property that does not hold.** `docs/API.md`'s tax-summary label table
says of `dividends_assessable`: *"The single column is unfranked (11S) + franked (11T) dividends
summed; split per the underlying income records"* — but the column also contains trust amounts, which
cannot be split into 11S/11T at all.

**Direction.** Trust rows are already identified (`income.trust_income`, and the annual report
separates them). Give them their own summary lines — franked → `13C`, unfranked → `13U` — mirroring the
`amma_*` lines already there, and leave `dividends_assessable` to company dividends so its documented
11S/11T split becomes true. `gross_assessable_investment_income` is unchanged: the same dollars move
between lines.

**Done.** `TaxYearSummary` gained two lines beside the `amma_*` pair they mirror —
`trust_income_unfranked` (**13U**, *share of net income from trusts less capital gains, foreign income
and franked distributions*) and `trust_franked_distributions` (**13C**, *franked distributions from
trusts*), both cited to `docs/ato/tax-return-labels-2026.md` and `docs/ato/amma-statement-guidance-notes.md`
Part B items 13U/13C. The income loop now routes a `trust_income` row's `unfranked_amount`/`franked_amount`
to those two instead of `dividends_assessable`, which is company dividends only — so its documented
`11S + 11T` split is finally true. Nothing else moved: the credits stay on `franking_credits`
(`11U / 13Q`, already correct for both routes), a trust row's CFI stays a memo *inside* the unfranked
amount (now inside the 13U line), attribution still follows `entitlement_date` over `date_paid`, AMIT-listing
rows are still excluded whole for their AMIT years (never re-routed here) while a converted fund's
pre-`amit_from` years report on the new lines, and `gross_assessable_investment_income` is unchanged —
the same dollars, a different line. Surfaces moved together: the field list, `CSV_HEADER`/`CSV_ATO_LABELS`,
`TaxYearSummaryCsv`'s `Cents` projection, the annual tax report's `tax_summary` block (which reads that
same label mapping), `COLUMN_KINDS` + `COLUMN_LABELS` in `src/web/util.js` (the headings carry the labels:
"Trust income, unfranked 13U (AUD)" / "Franked distributions from trusts 13C (AUD)"), the tax-summary
`REPORTS` description in `config.js`, and `docs/API.md` (a new *A trust distribution is question 13, not
question 11* paragraph, two new label-table rows, the corrected `dividends_assessable` row, and the
gross/CFI prose in both the Tax summary and Income sections). Tests:
`reports::tax_summary::tests::{db_a_trust_distribution_reports_at_question_13_not_with_company_dividends,
db_the_question_13_split_leaves_the_gross_assessable_total_unchanged,
db_a_trust_rows_conduit_foreign_income_stays_inside_its_13u_amount,
api_trust_and_company_income_report_on_separately_labelled_lines}`, the two new `label_of` assertions in
`db_ato_labels_align_with_their_columns`, and the existing trust/AMIT tests re-pointed at the new fields
(`db_trust_distribution_assessed_by_entitlement_date_not_payment`, `db_trust_entitlement_date_drives_fx_month`,
`db_full_year_mixed_income_types`, `db_a_converted_funds_pre_amit_years_are_still_reported`,
`db_amit_cash_rows_excluded_from_every_income_line`, `db_each_total_column_totals_the_columns_beside_it`,
`api_export_a_total_column_is_the_sum_of_the_columns_it_totals`,
`reports::tax_report::tests::{a_converted_funds_pre_amit_income_prints_behind_its_tax_summary_line,
conduit_foreign_income_prints_as_a_memo_column_and_is_not_double_counted}`,
`entities::drp_reinvestment::tests::the_partial_participation_workaround_costs_the_parcel_at_the_cash_reinvested`).

## SCENARIOS Z-d — a back-dated parcel leaves an AMMA statement's adjustment set stale, and nothing says so

- [x] Surface an AMMA statement whose `units_held` disagrees with the units actually held at its year end.

Found driving **Z-05** (the correction cascade). A year is entered, its AMMA statements are entered and
their [AMIT adjustments generated](../docs/API.md#generating-amit-adjustments), the tax report is
archived — and then a missed Buy dated **before** those year ends is discovered and entered. Every
other consequence is handled: all 15 report snapshots were marked stale by the schema's staleness
triggers, `regenerate_all` rebuilt them, and the archived FY2025 tax report came back **byte-identical**
(0 fields changed), which is right — the new parcel was never sold.

The AMIT side is not. Generation writes one row per parcel open at the statement's
`tax_year_end_date`; entering a parcel dated before that year end adds a parcel that set never saw:

| | statement `units_held` | Σ adjusted | units actually open at year end |
| --- | ---: | ---: | ---: |
| FY2024 statement, as generated | 1000 | 1000 | 1000 |
| after the back-dated 300-unit Buy | 1000 | 1000 | **1300** |

`GET /reports/amit_adjustment_cross_check` is **empty** in the second row, and so is
`/reports/health`. The check reconciles the adjustment *set* to the *statement* — Σ 1000 against
`units_held` 1000 — and by that measure it does reconcile. Nothing anywhere compares either figure
with the parcels actually open at the year end, so the 300 units keep their full cost base while the
fund's per-unit reduction was, per `docs/API.md`'s own rule, "applied uniformly to every unit held at
the statement's `tax_year_end_date`". The FY2025 statement is stale in the same way and equally silent.

**The control is what shows the check is blind precisely here.** Re-running generation with
`"replace": true` writes the corrected set (Σ 1300) and the cross-check fires immediately —
*"adjusted units 1300 exceed the statement's units held 1000 … (excess 300)"*. So the mismatch is
detectable and the report already knows how to say it; it is only ever seen when the set is
regenerated, which is the one action a user who does not know the set is stale has no reason to take.
Entering the same statement against the same holding **without** back-dating is flagged too (generation
covers 1300, `difference` 300). The blind spot is exactly the correction cascade: a parcel set that
changes *after* generation.

**Direction.** The cross-check already loads the open parcels it would need. Adding the statement's own
`units_held` versus the units open at `tax_year_end_date` as a third comparison — beside Σ-versus-
`units_held` — surfaces both a stale set and a statement typed against the wrong holding, and it says
which of the two figures moved.

**Fixed.** `reports::amit_adjustment_cross_check` grew a **fifth check**, beside the four that all
reconcile the adjustment *set* to the *statement*: the statement's own `units_held` against the units
actually open at its `tax_year_end_date`, **on its own listing and holding account**. The units come
from `domain::open_parcels::load(conn, Some(year_end))` — the same shared read
[generation](../docs/API.md#generating-amit-adjustments) derives its set from, so the report cannot
disagree with generation about what was open — one `load` per distinct year end, on the report's own
single `pool.begin()` read transaction. A new reported field `units_open_at_year_end` carries the
figure (classified `quantity` in `src/web/util.js`'s `COLUMN_KINDS`; the default humaniser labels it
"Units open at year end", so no `COLUMN_LABELS` entry). The sentence is:

> the statement states {units_held} unit(s) held at {year_end} but {units_open_at_year_end} unit(s)
> are open on its holding account at that date (difference {±diff}) — {cause}

and `cause` says **which of the two figures moved**: `the adjustment set still sums to the statement's
figure, so it is the parcels that changed after the set was generated — re-generate the set from the
statement (replace) once they are right` (the finding's own case); `the adjustment set already covers
the units that are open, so it is the statement's stated figure that disagrees — check it against the
registry's holding statement, and that it is the right holding account`; or, where neither agrees (or
there are no rows at all), `check the statement's units held and holding account against the parcels
entered[, then re-generate its adjustment set]`. There is deliberately **no allowance band** here,
unlike the coverage check: both terms are *year-end* positions, so units disposed of during the year
are already out of both. The check runs for every statement, including one with no adjustment rows —
it is a statement-level comparison, not a set-level one — and is pushed *last* so the existing
problems keep their order.

**Legitimate shapes driven, each pinned as a test that must stay unflagged by this comparison:**

| shape | what happens | test |
| --- | --- | --- |
| holding **sold out during the year** (F-04: statement states nil) | nil open at year end = nil stated → agrees | `db_a_statement_covering_units_sold_during_the_year_reconciles` (existing, now also pins this) |
| **partial sale during the year** | the statement states the year-end figure and that is what is open; the sold units are out of *both* terms | `db_a_partial_sale_during_the_year_is_not_flagged` |
| **share split** between acquisition and the year end | `load` re-bases the remainder into the year end's basis, `units_held` is already in it → split-aware in both terms | `db_a_split_does_not_false_positive_the_units_held_check` |
| **bonus issue** likewise | same re-basing path | `db_a_bonus_issue_does_not_false_positive_the_units_held_check` |
| **transfer after the year end** (the ordinary order — the statement arrives in spring) | the source parcel is still open at the year end, because its closing Sell is dated the transfer → agrees, while the row itself is written against the replacement (N-06) | `db_a_transfer_after_the_year_end_is_not_flagged` |
| **transfer during the year** | nil open on the statement's account at the year end = nil stated → this comparison stays quiet. (The row against the replacement parcel does trip the *coverage* check, whose disposal allowance is measured on the adjusted parcels and finds none on a replacement — pre-existing behaviour, not this finding's) | `db_a_transfer_during_the_year_is_not_flagged` |
| **scrip-for-scrip exchange during the year** | the whole holding of the statement's listing is consumed → nil open, nil stated → agrees. (Its replacements are of *another* listing, which `amit_adjustment`'s write-time `ListingMismatch` refuses outright, so such a statement carries no rows and the pre-existing "no adjustments entered" problem is what flags it) | `db_a_scrip_exchange_during_the_year_is_not_flagged` |
| **demerger during the year** | the head listing's replacement parcel carries the same units under the same listing, so the units are open at the year end all along → agrees | `db_a_demerger_during_the_year_is_not_flagged` |
| everything agrees | absent from the report — the "empty means everything reconciles" contract | `db_a_reconciling_set_is_not_flagged` (existing) |

**Tests that must flag:** `api_a_back_dated_parcel_entered_after_generation_is_flagged` — the finding's
own reproduction end to end through `ApiClient::full`: generate the FY2024 set (empty report), enter
the 300-unit Buy dated 2024-03-01, and the report now carries one row with `units_held` 1000,
`units_adjusted` 1000 and `units_open_at_year_end` **1300**, naming both figures, `difference +300`
and the stale-set cause. It goes on to pin the repair: `replace: true` moves the set onto the parcels
and the row *stays*, now saying the statement's own figure is the one left behind (the control from
the write-up), and correcting `units_held` to 1300 clears the report.
`db_a_statement_against_an_account_that_held_nothing_is_flagged` — the second thing the comparison
surfaces: a statement typed against holding account 2 when every parcel is in account 1 (0 open
against 1000 stated).

One existing fixture was corrected rather than tolerated: `db_a_mid_year_disposal_is_not_flagged`
stated `units_held` 509 for a holding that closed in February, which no registry would say at 30 June
— it now states nil, which is the realistic figure and still exercises exactly what that test is
about (the parcel-outside-the-year check not firing on a mid-year disposal).

Docs: `docs/API.md`'s AMIT adjustment cross-check section (four checks → five, the new bullet, the
field list, the "empty report" sentence) and the report's `REPORTS` description in
`src/web/config.js`.

## SCENARIOS Z-e — the archived CGT worksheet calls a bonus issue and a consolidation "splits", at ratios nobody announced

- [x] Name each unit-count event by what it was, at the ratio its terms were stated in.

Found driving **Z-08** (the rights round trip), which ends with a 1-for-10 **bonus issue** and a 1-for-2
**consolidation** over the same parcels. The [Annual Tax Report](../docs/API.md#annual-tax-report) — the
print document meant to be saved to PDF and archived — prints one `adjustments` row per event on every
disposed parcel, with a `reference` naming the action it came from. Both come out wrong:

| what was recorded | what the worksheet prints |
| --- | --- |
| `BonusIssue` 1 for every 10 held | `11-for-10 split` |
| `ShareSplit` 1 new for 2 old (a consolidation) | `1-for-2 split` |

`domain::cost_base::adjustment_detail` builds every one of these as
`format!("{}-for-{} split", s.new_units, s.old_units)` over the *derived rebase factor*, so:

- a **bonus issue** is not a split and was never announced as "11-for-10" — that factor is this tool's
  own arithmetic (10 held → 11 held), and a reader reconciling the worksheet against the company's
  announcement finds no such ratio in it;
- a **consolidation** is announced as "1-for-2" and that part is right, but calling it a *split* says
  the opposite of what happened — the parcel went from 2,200 units to 1,100.

The figures are all correct (`amount` is 0 — these rows are informational, explaining a changed unit
count, never a cost-base movement); it is the provenance label that misnames them, in the one document
that exists to be handed to someone else. `docs/API.md`'s worked example of the field is `"2-for-1
split"`, so the design only ever contemplated splits.

**Direction.** The rebase events already know which action kind they came from. Carry that through and
label each one from its own terms — `1-for-10 bonus issue`, `1-for-2 consolidation`, `2-for-1 split` —
rather than formatting one derived factor three ways.

**Fixed.** The announced terms now travel with the event.
`entities::corporate_action::adjustments::SplitEvent` grew a `terms: RebaseTerms` field beside the
existing `new_units`/`old_units` — an additive change, so every arithmetic caller
(`split_ratio`, `split_adjusted_quantity`, `as_acquired_quantity`, `RocEvent::per_unit_for`, the
report and write-time re-basing helpers) is untouched and the bonus issue still normalises into its
equivalent split for the rebase factor. `RebaseTerms` is the three-way enum the *label* reads:
`Split` / `Consolidation` (a `ShareSplit` split by whether `split_new_units < split_old_units`) and
`BonusIssue` (the announced bonus-for-held pair, not the derived 11/10). Two constructors,
`SplitEvent::share_split` and `SplitEvent::bonus_issue`, are the only way one is built — the row
loader (`split_event_from_row`, so both `db_share_split_events` and `db_splits_for_listing`) and the
test helpers all go through them, so the terms can never be dropped. `RebaseTerms::label` produces
`2-for-1 split`, `1-for-2 consolidation`, `1-for-10 bonus issue`; `domain::cost_base`'s
`adjustment_detail` calls it instead of formatting the factor. Terms are `normalize()`d, so terms
typed `2.00`/`1.00` read `2-for-1` (the old code printed `2.00-for-1.00`).

The degenerate `new_units == old_units` (a 1-for-1 `ShareSplit`) is representable — the write path
only requires both terms positive — and re-bases nothing. It is classified `Split`, with the reason
in a comment on the variant: "consolidation" would claim a unit count fell, so the no-op keeps the
action's own name and reads `1-for-1 split`.

Surfaces checked: `reference` is rendered only by the Annual Tax Report's `taxreport.js`, which passes
it through verbatim — no UI change. The listing-activity ledger (`reports::activity::describe_action`)
was already correct, and its wording is the precedent this follows.

**Tests:**
`entities::corporate_action::tests::db_split_events_carry_each_actions_announced_terms` — both
loaders, over a split, a bonus issue, a consolidation and a `2.00`/`1.00` split: the four labels, and
the *same* rebase factors (2/1, 11/10, 1/2) and re-based quantity as before.
`domain::cost_base::tests::rebase_rows_are_named_by_their_own_kind_and_announced_terms` — the five
`reference` strings out of `adjustment_detail`, all still nil-amount informational rows.
`domain::cost_base::tests::a_bonus_issue_and_a_consolidation_rebase_by_their_derived_factors` — the
regression that matters: a return of capital either side of both events still reduces by 11c per
as-acquired unit each time.
`reports::tax_report::tests::api_a_bonus_issue_and_a_consolidation_are_named_by_their_own_terms` —
end to end through `POST /reports/tax-report` via `ApiClient::full`, asserting the two labels on the
disposed parcel's `adjustments` and that its units, cost base, proceeds and gain are unchanged.

Docs: `docs/API.md`'s `disposals` bullet now states the rule and gives all three labels.

## SCENARIOS Z-a — one disposal's gain prints as two different figures on two screens

- [x] Make a sale's own proceeds and gain the exact figures, not the sum of re-rounded shares.

Found driving **Z-01** (the 10-year ETF), whose closing sale spans 10 parcels. Measured in a real
browser on the two screens a user would compare:

| screen | what it prints for the 2026-05-15 disposal |
| --- | --- |
| Realised Gains (`#/r/realised-gains`) | proceeds `69,785.05`  cost base `39,139.98`  **gain `30,645.07`** |
| Annual Tax Report (`#/r/tax-report`) | *"Subtotal: proceeds 69,785.05, cost base 39,139.98, **gain/loss 30,645.08**"* |

The exact figure is `69,785.05 − 39,139.975 = 30,645.075`, so the tax report is right and the
Realised Gains cell is a cent low. **The same row disagrees with itself**: its discount-eligible and
non-discountable columns print `30,316.38` and `328.70`, which add to `30,645.08` — the cent the gain
cell beside them does not show. That is W-d's "printed columns do not add up", on screen this time.
The [net capital gain](../docs/API.md#net-capital-gain) report agrees with the tax report.

**Mechanism.** `reports::realised_gains` never computes a sale's total: `sale_proceeds` is accumulated
from the per-allocation shares. Each share is `sale.average_price × qty_alloc − alloc_costs`, and
`alloc_costs` — the pro-rated brokerage — is deliberately a *cumulative difference* so the shares
telescope to exactly `brokerage + gst`. They do. What breaks the telescoping is the **subtraction that
follows**: `price × qty` is a large exact number and `alloc_costs` a 28-significant-digit repeating
one, so each difference is re-rounded to fit `Decimal`'s mantissa and the residues no longer cancel.
The report's own test comment names the hazard ("a larger price would re-round there") but only for
the shares, not for the total.

**Reproduced with three controls agreeing** (parcels of equal size, sale brokerage 9.95):

| case | exact proceeds | reported |
| --- | ---: | --- |
| 3 parcels × 517u @ 45.00 | 69,785.05 | `69785.049999999999999999999999` |
| 3 parcels × 517u @ **4.00** | 6,194.05 | `6194.0499999999999999999999999` |
| 3 parcels × 517u, **no brokerage** | 69,795.00 | `69795.00` ✔ |
| **1 parcel** × 1551u @ 45.00 | 69,785.05 | `69785.05` ✔ |

So it is the apportionment, not the magnitude: the drift appears whenever the brokerage share is a
repeating decimal, and disappears when there is nothing to apportion or nothing to apportion it
across. Whether it changes a *displayed* cent then depends on where the exact figure sits — Z-01's
landed on a half-cent, which is what made it visible.

**Direction.** The sale-level `proceeds` is knowable exactly (`price × quantity − brokerage − gst`,
converted once); computing it there and letting the last allocation absorb the difference keeps both
properties W-d established — the total is exact, and the per-parcel rows still sum to it.

**Fixed** in `reports::realised_gains::compute_realised_gains`: an allocation's proceeds is now a
share of the sale's *own* net proceeds — `average_price × quantity − brokerage − gst`, converted to
AUD **once** at the sale's single rate — apportioned by quantity through a new shared
`cumulative_share` helper, instead of `price × qty_alloc` less a separately pro-rated brokerage
share. Apportioning the whole figure is what removes the re-rounding: the old form subtracted a
28-significant-digit repeating share from an exact large number, and every such subtraction rounds to
fit Decimal's mantissa, so the residues stopped cancelling. `cumulative_share` keeps *both* of W-d's
properties at once — the differences telescope, so the shares sum to the total, and the allocation
that completes the disposal (`units_so_far == units_total`) is handed the whole total with no
division at all, so not even the final `total × units / units` can re-round it. Converting once
rather than per allocation is equivalent because every allocation of a sale converts at the same
rate, and it removes one rounded division per allocation.

The **`RightsSale`** accumulator had the same class of defect by a different route: nothing there is
apportioned and then combined with an exact figure, but each allocation's two legs were converted to
AUD separately, so every share carried its own rounded division. Measured: US$0.20 × 3 rights at 0.60
USD/AUD summed to `0.9999999999999999999999999999` against an exact A$1.00, and the US$10.00 carried
cost to one ulp under 10/0.6. Both totals are converted once now and apportioned by units through the
same helper.

Rounding *policy* is untouched — `infra::decimal::to_cents`, the CSV exports and the annual tax
report's sum-of-rounded-rows convention (W-d) all behave exactly as before; only the exact figure the
report computes changed.

**Tests** (all five fail on the pre-fix code; the three controls pass either way, which is what says
it was the apportionment):
`reports::realised_gains::tests::pure_ten_parcel_disposal_reports_its_exact_proceeds_and_gain` — the
finding's own shape: 1,551 units over 10 parcels at $45.00 less $9.95, asserting proceeds exactly
`69785.05`, cost base exactly `39139.975`, gain exactly `30645.075`, `to_cents` of the gain
`30645.08`, and that the 10 per-parcel rows still sum to each total exactly.
`…::pure_equal_parcel_shares_sum_exactly_at_a_large_price` / `…_at_a_small_price` — 3 × 517 units at
$45.00 and at $4.00 (69,785.05 and 6,194.05, previously `…049999999999999999999999` and
`…0499999999999999999999999`).
`…::pure_equal_parcel_shares_sum_exactly_without_brokerage` and `…::pure_single_parcel_proceeds_are_
exact` — the two controls that were always exact: nothing to apportion, and nothing to apportion it
across.
`…::pure_non_aud_multi_parcel_proceeds_convert_the_exact_total_once` — a USD 3-parcel disposal pinning
proceeds, cost base and gain.
`…::pure_non_aud_rights_sale_shares_sum_exactly_to_the_converted_totals` — the rights regression.
`…::api_one_disposal_reports_one_rounded_gain_on_both_screens` — the two-screens comparison end to
end through `ApiClient::full`: `GET /portfolio/realised-gains` and `POST /reports/tax-report` must
print the same cents for one 3-parcel disposal whose gain is exactly `30981.875` (before the fix:
`30981.87` against `30981.88`).
`…::pure_brokerage_shares_sum_exactly_across_allocations` keeps its $2 assertion; its comment no
longer has to warn that a larger price would mask it.

Docs: `docs/API.md`'s "Where a Sell's brokerage and GST land" paragraph now states that a disposal's
`proceeds` is exactly `price × quantity − brokerage − GST` however many parcels it was allocated
across, and that the `parcels` rows still sum to it exactly.

## SCENARIOS AA-d — a disposal recorded at nil proceeds raises a capital loss that nothing questions

- [x] Decide and implement (options below).

Scenario AA-03. A gift of shares is a CGT disposal at **market value** under the market-value
substitution rule, and `docs/API.md` documents the entry convention: "enter a gift out as a manual
Sell at market-value proceeds". The failure mode the convention exists to prevent is entering what
was actually *received* — nothing — and that entry is accepted in full:

```
PUT /sells/71  {"average_price":"0","quantity":"1000", ...}   → 204
```

`GET /portfolio/realised-gains` then reports `proceeds: 0`, `cost_base: 20000.00`,
`capital_loss: 20000.00`. **A$20,000 of capital loss that does not exist**, feeding the net-capital-
gain netting and the 18V carry-forward. The health report is silent — its only non-empty lists after
the write were `unpriced_days` and an unrelated `duplicate_income`.

The system cannot know a nil-proceeds Sell is a gift, so this is a flag rather than a refusal — and
a nil-proceeds disposal is a genuinely unusual shape worth naming, in the way `duplicate_trades` and
`non_trading_day_trades` already are. (The one legitimate nil-proceeds disposal — worthless shares —
has its own operation and writes a Sell carrying `worthless_action_id`, so it is distinguishable; a
crypto burn is the residual honest case.)

**Options.**

1. **A health check** — `nil_proceeds_disposals`, listing every ordinary Sell and rights sale
   recorded at nil proceeds (excluding the operation-written closing Sells), with the
   market-value-substitution rule as its reason. Advisory, blocks nothing.
2. **Documentation only** — extend the *Gifts / off-market related-party transfers* bullet to warn
   that entering the nil consideration actually received fabricates a capital loss, and say so on
   the Sells screen.
3. **Out of scope** — a nil-proceeds disposal is legitimate often enough (a crypto burn, an
   abandonment) that naming it would be noise.

**Chosen: option 1 — a `nil_proceeds_disposals` health check.**

**Fixed.** `reports::health` now carries `nil_proceeds_disposals`: every ordinary Sell at a zero
`average_price`, plus every rights disposal at a zero `proceeds_per_right` whose rights were **paid
for** (`rights_cost > 0`) — a *free* right lapsing is nil against nil, the non-event `docs/API.md`
describes, and flagging it would fire on every ordinary lapse. The test is the *price*, not the
netted proceeds: a real price a brokerage happens to cancel is arithmetic, not a nil-consideration
disposal, and the market-value substitution rule has nothing to say about it. Advisory, blocking
nothing, with the rule (`docs/ato/capital-proceeds-market-value-substitution.md`, QC 66021) as its
reason, a cross-view banner sentence linking to Sells / Rights Sales, and the Gifts limitation,
the Health field list and the README feature line all updated (pinned by
`doc_checks::nil_proceeds_disposals_are_documented_with_the_market_value_rule`).

The exclusion of the operation-written closing Sells is the part that needed the rule rather than a
list. There were already **three** transcriptions of the provenance columns (the two guards in
`entities::sell`, and the write-path `CASE` in `non_trading_day_trades`), so the exclusion became
`entities::trade::provenance` — one list of (column, plain-English write path) with two SQL
builders over it, `operation_written_sql` and `source_case_sql`, and a test that reads the live
schema's foreign keys on `trades` and fails on one that is neither classified as a provenance link
nor named as ordinary trade data with the reason. A future operation's column is picked up by both
callers with no edit. `non_trading_day_trades` now builds its label from the same list, which also
fixed a mislabel it carried: a crypto transfer's network-fee Sell (linked from
`transfers.fee_sale_trade_id`, not `trades.transfer_id`) read as `entered directly`.

## SCENARIOS AA-f — the archived CGT worksheet prints a whole parcel's initial cost against a part of it

- [x] Decide and implement (options below).

Reported in passing by the agent fixing [AA-a](tax-domain.md#scenarios-aa-a--an-indexation-eligible-parcel-is-silently-costed-on-the-discount-and-the-reason-given-for-not-modelling-it-is-false-for-a-wide-enterable-range), and **re-driven from scratch against a
throwaway database before being logged** (per the standing lesson: a fixing agent's incidental report is
re-derived, not taken on trust). The reproduction is real and the mechanism is as reported.

`reports::tax_report` takes `CostBase::initial_cost` — the **whole parcel's** figure — for a disposal
row's `initial_cost_base_aud`. Sell 500 units of a 1,000-unit A$10 parcel and the Annual Tax Report's
disposal schedule prints:

| Units | Buy price | Initial cost base (AUD) | *(adjustment rows)* | Adjusted cost base (AUD) |
| ---: | ---: | ---: | --- | ---: |
| 500 | 10.00 | **10,000.00** | *(none)* | **5,000.00** |

with `cost_base_per_unit_aud` of `10.00` beside it. A hand-checker multiplies 500 × $10, gets $5,000,
and finds an "Initial cost base" of $10,000 with **nothing between the two columns explaining the
difference** — which is precisely the contract `docs/API.md` states for this section:

> the initial cost base and, **itemised underneath it, one row per cost-base adjustment** … with its
> own date, reference, and per-unit figure

**Bounded with its control**: a disposal of the *whole* parcel prints correctly (1,000 units → initial
`10,000.00`, adjusted `10,000.00`). The fault appears only on a **partial** disposal, which is the
ordinary case for any holding sold down in tranches.

**No tax figure is wrong.** `initial_cost_base_aud` is a display column, not one of the five the
section totals — the subtotal, the gain and the discount all take the adjusted figure. But this is the
print document meant to be saved to PDF and archived, and a column that does not reconcile against the
units beside it is exactly the class of fault [W-c](reporting.md) and [W-d](reporting.md)
were about: *a column has to add up on the page*.

The AA-a commit (`369e040`) added `CostBase::costed_initial_cost`, which is precisely the figure this
row wants, so option 1 is a small change — but it changes a **printed number**, which is why it was not
folded into that commit.

**Options.**

1. **Print the costed units' initial cost.** `initial_cost_base_aud` becomes `costed_initial_cost`, so
   the row reads 500 units / initial `5,000.00` / adjusted `5,000.00`, and where adjustments exist they
   account for the whole of the gap — restoring the documented contract. A previously archived PDF will
   disagree with a freshly generated one for the same year in this one column.
2. **Keep the figure, fix the label.** Rename the column to say it is the parcel's, not the disposal's
   (and say so in `docs/API.md`), leaving archived documents reconcilable against new ones.
3. **Out of scope** — a display column that no total depends on.

**Chosen: option 1 — print the costed units' initial cost.**

**Fixed.** `reports::tax_report`'s disposal row now takes `CostBase::costed_initial_cost` — the costed
units' pro-rated share of the parcel's initial cost base, the same pool `cost_base::adjustment_detail`
starts its itemised walk from — instead of the whole parcel's `initial_cost`. The reproduction row now
prints 500 units / initial `5,000.00` / adjusted `5,000.00`, and the whole-parcel control is unmoved at
1,000 / `10,000.00` / `10,000.00`. With real adjustments present the documented contract holds as an
arithmetic identity: a 400-of-1,000-unit disposal of an A$10 parcel carrying a 50c/unit AMIT reduction
prints initial `4,000.00` − `200.00` = adjusted `3,800.00`, and the same parcel carrying a 25c/unit
return of capital prints `4,000.00` − `100.00` = `3,900.00` (the identity holds except where a row is
flagged `capped` and CGT event E10/G1 has floored the balance at nil — the excess is a capital gain in
the net-capital-gain report, not a cost-base movement). The rest of the row was swept for the same
fault and is correct: `adjusted_cost_base_aud`, `proceeds_aud`, `gain_loss_aud` and the two per-unit
figures all come from the allocation (`realised_gains::ParcelDetail`), the itemised adjustment amounts
and per-unit figures are already stated for the costed units, and `indexed_cost_base_aud` was built on
`costed_initial_cost` from the start (`domain::indexation::indexed_cost_base`). The one other
whole-parcel figure is `buy_brokerage`/`buy_gst_on_brokerage` — deliberately the buy contract note's
own figures for the whole trade, transcribed for checking against the note, carried in the JSON and
printed in no column; that is now said in the field's doc comment and in `docs/API.md` rather than left
to be inferred. Rounding is untouched (the column was already in `round_money_to_cents`'s list), and
nothing totalled moved: driven end to end at the HTTP surface against a throwaway database, the whole
document — every subtotal and grand total included — is byte-identical to the pre-fix binary's apart
from this one column. Because it changes a printed number, `docs/API.md` says so where the column is
described, so a reader comparing an archived PDF against a freshly generated one is not left guessing.
Regression tests:
`reports::tax_report::tests::api_a_partial_disposal_prints_the_disposed_units_initial_cost_base` (the
partial disposal and the whole-parcel control in one document) and
`api_the_itemised_adjustments_span_the_whole_gap_on_a_partial_disposal` (the identity, over both
reduction kinds).

<!-- Closed 2026-08-27 (6ae6d45 the research gate, 16f9eb1 the build, 849bb83 the docs and
the live-provider verification). Archived here rather than in infra.md because the deliverable is
two health-report alerts; the table and the import job exist to feed them. -->

## Distribution calendar and the missing-dividend alert (REQUIREMENTS 2026-08-27, narrowed same day)
(Advisory data-completeness only — the feed must never gate a tax figure. Narrowed from the first
draft after the AMMA coverage fix landed on recorded facts alone: the third `amma_missing` limb and
the resolution of the advisory `amma_nothing_recorded` list are both **cut**, with reasons in
REQUIREMENTS' "Deliberately out of scope". Provider capability verified live against `yfinance-rs`
0.9.1 on 2026-08-27; three measured facts constrain the work — `Range::Max` silently truncates the
action stream (VDHG 8 events vs 28 for the same span as an explicit period), so the fetch must pass
an explicit `between(start, end)`; the stored date is the **ex-date**, which for an ASX fund's
June-half distribution falls a day or two into July while the income is attributed to the year just
*ended*, so events are matched to income rows by event and never bucketed into a financial year;
and matching cannot key on `ex_date`, since 13 of the live database's 47 income rows have none.)
- [x] **Gate on this first, before any code**: settle whether Yahoo's ASX ETF coverage is complete.
  It returns 8 HNDQ events since the August 2020 launch where a semi-annual payer should have 11–12
  (2022-01, 2022-07, 2023-07, 2026-01 absent). Compare against Betashares' published HNDQ
  distribution history and record the answer in REQUIREMENTS. If the coverage is holed, "no ex-date
  found" cannot mean "no distribution" and the alerts below can only ever fire on events Yahoo
  *does* have — still useful, but say so rather than implying completeness
  — **settled 2026-08-27: the coverage is not holed.** Betashares' own distribution table (read out
  of the raw HTML, not a rendering) prints a bare `-` in its amount column for exactly the four
  periods Yahoo lacks, so HNDQ distributed nothing on them; its other eight rows match Yahoo's
  eight events to 6 dp. Recorded in REQUIREMENTS under "Coverage settled", with the two limits on
  how far one security generalises. **The gate is clear** — the alerts below may read "no ex-date
  found" as "no distribution"
- [x] **Found while settling the gate, and it corrects a fact recorded in REQUIREMENTS earlier the
  same day**: `Action::Dividend.date` is a **UTC** calendar date, not the ex-date — one day early
  for every ASX event in AEDT (October–April), where it then routinely lands on a day the market
  was shut (New Year's Day, a Sunday, Easter Monday). The fetch must recover the true ex-date by
  joining the event to the candle sharing its UTC date (`fetch_full()` returns candles and actions
  from one response, and `Candle::ts` keeps the instant the action lost) — verified 10 of 10
  against issuer-published dates across HNDQ, BHP and VDHG. See REQUIREMENTS "The one-day ex-date
  shift" and "The correction, verified"
  — done in `entities::distribution_event::yahoo` (`16f9eb1`), and proved in production rather than
  only in tests: `POST /jobs/distribution-import` against the live provider stored all 8 HNDQ events
  at exactly Betashares' published ex-dates, four of them the AEDT ones the crate reports a day
  early (`849bb83`)
- [x] `distribution_events` table + migration (listing, ex-date, amount per unit, currency,
  provenance); classify it for snapshot staleness and `row_history` auditing per CLAUDE.md
  — migration 0048: audited (the 23rd table, with the `row_history` CHECK rebuild its addition
  needs) and classified **staleness-exempt**, since the three snapshotted reports are the
  price-dependent ones and its only reader, `reports::health`, is computed live
- [x] Provider-agnostic fetch behind a trait, Yahoo the only provider-specific part, on the
  `closing_price` pattern; explicit period, never `Range::Max`; candle-joined ex-date, never the
  raw `Action::Dividend.date`
  — a **third** measured provider fact turned up while building the cross-check and inverted a
  default: Yahoo restates a security's whole dividend history into the **current** unit basis,
  cumulatively (NVDA's pre-split dividends come back as 0.004 against a declared $0.04, and the ones
  before its 2021 4-for-1 as 0.004 too, against a declared $0.16). So `amount_per_unit` is stored in
  the basis of its own `fetched_at`, exactly as served — a conversion back could only use the splits
  recorded at fetch time and would be silently wrong for any recorded later — and the reader
  multiplies it by units in that same basis, a total being basis-independent
  (`HeldTimeline::units_by_account_on` takes the basis as its own parameter). Recorded in
  REQUIREMENTS and `docs/SCHEMA.md`
- [x] Scheduled refresh job in `infra/scheduler/registry.rs` + its `schedule.cron` line
  — `distribution-import`, weekly (Monday 05:00). One provider call per held listing over the span
  it was held; it never deletes, and an event it cannot place on the market's calendar qualifies the
  run with a note rather than vanishing
- [x] `reports::health` **missing dividend entry** alert: known ex-date, units held on it, no
  matching income row — carrying ticker, ex-date and expected amount (per unit × units held)
  — one deliberate departure from the wording, stated because it changes the answer: held is
  measured on the **last cum-dividend day**, the day *before* the ex-date, since that is what
  entitles a holder. A Buy dated on the ex-date bought the security without the distribution
  attached, and counting it would invent an entitlement; the test carries the control (the same Buy
  one day earlier does fire the alert). Matching is per **holding account** — an entitlement is paid
  to a registered holder — on the income row's own `ex_date` where it has one and otherwise a
  −15/+45-day window over `entitlement_date`/`date_paid`, with no row claimed by two events
- [x] `reports::health` **amount cross-check** alert: known ex-date matched to an income row whose
  gross cash differs materially from per unit × units held. Gross total only, never components —
  this is the likelier error of the two and the one the 6 dp reconciliation shows Yahoo can catch
  — "materially" is 2% of the expected gross **and** at least $1: the band absorbs registry rounding
  while a mistyped figure sits far outside it, and the floor stops a fraction-of-a-cent per-unit
  distribution (HNDQ paid 0.018741) alerting on cents
- [x] Docs: `docs/SCHEMA.md` (table + relationships), `docs/API.md` (both alert shapes), README
  Features
  — plus a `docs/API.md` **Distribution calendar** section and a Known-limitations entry saying what
  an alert *not* firing does not prove, all pinned by
  `doc_checks::distribution_calendar_documented`

## REQUIREMENTS: annual tax report — foreign income totals (2026-08-28)

The Foreign income table gathers every foreign amount the year produced, but its four kinds are
reported in three different places, so the column as a whole totals to nothing anyone transcribes.
Asked for: the subtotals a reader actually needs, printed under the table without a paragraph
explaining them.

- [x] `income.foreign_income_totals` on the annual tax report — three lines, each an amount and the
  foreign tax paid on it: `non_amma` (the dividend/trust and interest rows — question 20's gross,
  the tax summary's `foreign_source_income` + `foreign_interest_income`), `amma` (the AMIT's
  attribution — its own `amma_foreign_income` line, its foreign tax a FITO credit rather than tax
  the taxpayer paid), and `total`, the two together
  — the ESS row is in none of the three: its amount is already inside the item 12 discount, so
  totalling it would report the same dollars twice. That is the one thing the printed page does not
  say in words — the row's own *Kind* cell names it a memo, and the reason lives in `docs/API.md`
  and on the type
- [x] `ForeignIncomeRow.kind` typed as `ForeignIncomeKind` rather than free text, its serialized
  names unchanged (`#[serde(rename)]`), so the printed *Kind* column and the JSON read exactly as
  before
  — which line a row belongs to is a tax question, not a string comparison; the enum is what makes
  `ForeignIncomeTotals::of` answer it in a `match` the compiler checks
- [x] **Summed at the cent** — the rule every total this document prints beside the figures it
  totals follows (SCENARIOS W-f, `tax_summary`'s own total column), so each line is what a reader
  gets adding the rows it covers
  — the rows themselves stay exact and still sum exactly to their summary lines, so a line can sit
  up to a cent from the exact sum of those. Pinned by `foreign_income_totals_add_the_rows_as_printed`,
  whose two rows land on a half cent: added as printed they make 30.02, added exactly 30.01
- [x] Printed under the table in the web document as three plain lines — two `subtotal`, one
  `total`, the same classes the disposal schedule uses (`foreignIncomeTotals` in `taxreport.js`,
  pinned in `web::tests::annual_tax_report_ui_present`)
- [x] Docs: the `income` bullet in `docs/API.md` (incl. the second deliberate exception to "every
  AUD figure sums exactly to its tax-summary line" — totals of rows, struck at the cent), README
  Features, REQUIREMENTS; pinned by `doc_checks::foreign_income_totals_documented`
  — verified end to end as well as by unit test: a seeded year carrying all four kinds prints
  100.00 + 40.01 under a non-AMMA subtotal of 140.01, the AMMA row's 70.00 under its own, and 210.01
  as the total, with the ESS memo in the table and in none of the three

## REQUIREMENTS: annual tax report — the version that produced the document (2026-08-28)

An archived PDF is the only record of what a year's figures *were*: nothing is stored and no year is
ever closed, so a re-run always recomputes from today's facts and today's rules.

- [x] `meta.app_version` (`env!("CARGO_PKG_VERSION")`), printed on the document's provenance line
  under the heading — `Produced <timestamp> · <taxpayer basis> · share-tracker v<version>`
  — the version answers what the timestamp cannot: a rule this system has since corrected (the LIC
  halving, the partial-disposal initial cost base) moves a printed figure with **no input having
  changed**, so a PDF disagreeing with a fresh run is only diagnosable if it names the code. Pinned
  by `meta_names_the_version_that_produced_the_document` and the bundle assertion in
  `web::tests::annual_tax_report_ui_present`
- [x] Docs: the `meta` bullet in `docs/API.md`, and the "a lodged financial year can be restated"
  limitation — whose remedy is "save the PDF at lodgement and compare" — now says the archived copy
  stamps its version, so a later disagreement can be told from a changed rule as well as changed
  facts; pinned by `doc_checks::tax_report_version_stamp_documented`
