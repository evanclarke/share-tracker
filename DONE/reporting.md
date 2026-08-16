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

Docs: the [AMIT cash cross-check](docs/API.md#amit-cash-cross-check) section states the per-account
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
