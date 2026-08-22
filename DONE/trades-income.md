# Done — Trade & Income Recording — Buys, Sells, DRP, AMMA, Attachments

## Trade Activity
- [x] Trade model (type, date, settlement date, listing FK, average price, quantity, currency, brokerage, GST on brokerage, brokerage currency, FX rate, contract note reference)
- [x] DB schema: `trades` table
- [x] Auto-populate settlement date from trade date + exchange settlement period (overridable)
- [x] CRUD API endpoints for trades
- [x] Tests: buy, sell, DRP trades; settlement date auto-population; override of settlement date
- [x] Refactor financial fields (average_price, quantity, brokerage, gst_on_brokerage, fx_rate) from f64 to Decimal
- [x] Tests: decimal precision preserved in API round-trip

## Income Activity
- [x] Income model (listing FK, date paid, ex date, franked amount, unfranked amount, foreign source income, foreign tax paid, TFN withholding tax, franking credits, LIC capital gain deduction, conduit foreign income, trust income flag, reinvestment trade FK)
- [x] DB schema: `income` table
- [x] CRUD API endpoints for income
- [x] Tests: dividend income, trust distribution, DRP reinvestment linkage
- [x] Refactor financial fields (franked_amount, unfranked_amount, foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, lic_capital_gain_deduction, conduit_foreign_income) from f64 to Decimal
- [x] Tests: decimal precision preserved in API round-trip

## AMMA Statements
- [x] AMMA model (listing FK, tax year end date, units held, date received, australian interest, australian dividends unfranked, franked dividends, franking credits, net rent, foreign income, foreign tax credits, other income, CGT discount gains, CGT indexation gains, CGT other gains, capital losses applied, tax deferred amount, tax free amount, cost base adjustment per unit, TFN withholding tax)
- [x] DB schema: `amma_statements` table
- [x] All financial fields use Decimal (not f64): australian_interest, australian_dividends, franked_dividends, franking_credits, net_rent, foreign_income, foreign_tax_credits, other_income, cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax
- [x] CRUD API endpoints for AMMA statements
- [x] Tests: insert and retrieve AMMA statement; cost base adjustment calculation
- [x] Tests: decimal precision preserved in API round-trip

## Share Parcel Allocation
- [x] Parcel allocation model (sale trade FK, purchase trade FK, quantity allocated)
- [x] DB schema: `parcel_allocations` table
- [x] quantity_allocated uses Decimal (not f64)
- [x] Validate quantity allocated does not exceed available quantity on purchase trade
- [x] Validate total allocations for a sale trade do not exceed sale quantity
- [x] CRUD API endpoints for parcel allocations
- [x] Tests: allocation creation, over-allocation rejection
- [x] Validate sale_trade_id references a trade of type Sell
- [x] Validate purchase_trade_id references a trade of type Buy or DRP
- [x] Tests: type constraint violations rejected

## DRP (Dividend Reinvestment Plan)
- [x] DRP Enrolment model (listing FK, residual handling enum: CarryForward/PayOut) — `src/entities/drp_enrolment.rs`, struct `DrpEnrolment` keyed by `listing_id`, enum `ResidualHandling` (defaults CarryForward)
- [x] DB schema: `drp_enrolments` table; `listing_id` PRIMARY KEY + FK→listings (at most one enrolment per holding); CHECK constraint on residual_handling enum; default CarryForward — migration `0013_drp_enrolments.sql`
- [x] CRUD API endpoints for DRP enrolments — `/drp_enrolments` and `/drp_enrolments/:listing_id` (GET/PUT/DELETE); a bad listing FK on PUT → 422
- [x] Tests: insert/retrieve enrolment; one-per-listing uniqueness enforced; residual_handling enum constraint enforced; FK to listing (`db_insert_and_retrieve`, `db_one_enrolment_per_listing_upsert_updates`, `db_residual_handling_enum_constraint_enforced`, `db_listing_fk_enforced`, plus API tests)
- [x] Trade Activity: add residual_brought_forward, residual_carried_forward, residual_paid_out columns (DRP trades only) — Decimal stored as TEXT; migration `0012_trade_drp_residuals.sql` (ALTER TABLE ADD COLUMN, defaults '0', no data dropped)
- [x] Tests: residual fields round-trip with decimal precision preserved (`trade::tests::db_drp_residual_fields_round_trip_with_precision`, `db_non_drp_trade_defaults_residuals_to_zero`)
- [x] DRP reinvestment operation: create a DRP trade from a distribution (Income Activity) + reinvestment price — `src/entities/drp_reinvestment.rs` `db_reinvest`: reinvestable cash (`franked+unfranked+foreign_source−foreign_tax−tfn`, excludes franking credits) + residual brought forward (latest prior DRP trade's carried-forward for the listing, else 0) = available; quantity = floor(available / price); cost = quantity × price; leftover → carried-forward or paid-out per the enrolment's residual handling
- [x] DRP reinvestment is atomic: `POST /income/:id/reinvest` creates the Trade (Type DRP, listing + currency + pay date from the distribution, quantity, average price = reinvestment price, residual fields) and sets the distribution's reinvestment_trade FK in one transaction; returns 201 with the trade
- [x] Validation: reject (422) reinvestment for a non-enrolled holding, or a distribution that already has a reinvestment trade (at most one per distribution); also 422 on non-positive price, 404 on missing income
- [x] Tests: carry-forward residual is picked up by the next reinvestment for the holding; pay-out records leftover as paid out (not carried); whole-share floor; reinvestable cash excludes franking credits; atomic trade-creation + distribution linkage; rolled back on failure; rejected when not enrolled / already reinvested (`drp_reinvestment::tests`: `carry_forward_buys_whole_shares_and_carries_leftover`, `carried_residual_is_picked_up_by_the_next_reinvestment`, `pay_out_records_leftover_as_paid_not_carried`, `franking_credits_are_excluded_from_reinvestable_cash`, `not_enrolled_is_rejected_and_nothing_persisted`, `already_reinvested_is_rejected`, `missing_income_is_not_found`, `non_positive_price_is_rejected`, plus API tests)

## Document Attachments
Attach supporting documents (trade confirmation PDF, dividend statement, AMMA scan) to a Trade, Income, or AMMA Statement. Contents are stored in the DB as a BLOB so the existing weekly backup captures them. Binary payload, so the API is not the JSON-CRUD convention (multipart upload, raw-bytes download, metadata-only list/get).
- [x] Add `sha2` dependency for the SHA-256 content checksum
- [x] Attachment model — `src/entities/attachment.rs`, struct `Attachment`: id, exactly-one owner FK (`trade_id` / `income_id` / `amma_statement_id`, others null), `filename`, `content_type` (enum `ContentType` over the MIME allowlist; `sqlx::Type` + serde `rename` so it round-trips as the MIME string), `byte_size`, `checksum` (SHA-256 hex via `checksum_hex`), `uploaded_at`. The `content` BLOB is absent from this metadata struct — loaded only by `db_get_content` for the download path
- [x] DB schema: `attachments` table — three nullable owner FK columns, each `REFERENCES <activity>(id) ON DELETE CASCADE`; `CHECK` that exactly one owner is non-null (`(trade_id IS NOT NULL) + (income_id IS NOT NULL) + (amma_statement_id IS NOT NULL) = 1`); `content_type` CHECK enum (`application/pdf`, `image/png`, `image/jpeg`); `content` BLOB NOT NULL; `byte_size` INTEGER, `checksum`/`filename`/`uploaded_at` TEXT — migration `0004_attachments.sql`. The pool already sets `foreign_keys(true)` (`infra::db::init`), so the cascade fires
- [x] Upload endpoint: `POST /attachments` multipart/form-data (file + target owner) → 201 with metadata; computes `byte_size` + `checksum` server-side; unsupported/absent content type → 422, missing/>1 owner → 422, unknown owner activity (FK violation via `write_error_status`) → 422, oversized (> 25 MB `MAX_UPLOAD_BYTES`) → 413. The route's `DefaultBodyLimit` is raised above the per-file cap so the explicit size check (not axum's default 2 MB limit) is what returns 413
- [x] Metadata list/get: `GET /attachments` (filterable by owner via `?trade_id=` / `?income_id=` / `?amma_statement_id=`, built with `QueryBuilder`) and `GET /attachments/:id` select only the metadata columns — the blob is never returned
- [x] Content download: `GET /attachments/:id/content` streams the raw bytes with the stored `Content-Type` and a `Content-Disposition` filename (quotes/backslashes stripped); 404 if unknown
- [x] Delete endpoint: `DELETE /attachments/:id` → 204, or 404 if unknown
- [x] Cascade: deleting a Trade / Income / AMMA Statement removes its attachments automatically (`ON DELETE CASCADE`), so no orphaned blobs remain
- [x] README sync: `attachments` table in the Database schema + Relationships sections; an Attachments section in the HTTP API; 413 added to Response codes (and the 201/422 rows extended); web-frontend paragraph mentions the Attachments action
- [x] Tests (`attachment::tests`): upload returns 201 + metadata and stores content; checksum + byte_size computed correctly; round-trip download returns the exact bytes with the stored content-type + filename; content type outside the allowlist → 422; missing-owner and two-owners → 422; unknown owner activity → 422; oversized upload → 413; metadata list excludes the blob + filters by owner; deleting the owning activity cascades; delete unknown → 404; plus DB-level exactly-one-owner and content_type CHECK enforcement

## Cost Base Adjustments
- [x] AMIT cost base adjustment: apply AMMA `tax deferred` amounts to reduce cost base of affected parcels
- [x] Tests: AMIT adjustment
- [x] CGT event E10: when cumulative AMIT cost base reductions on a parcel exceed its cost base, floor the cost base at nil (never negative) and report the excess as a capital gain in the AMMA statement's income year — cost base floored in the portfolio/unrealised/realised reports (`(initial_cost - amit).max(0)`); `net_capital_gain::e10_gains` walks each parcel's adjustments in tax-year order, emits the per-year excess (converted to AUD at the parcel's buy-month rate), classifies it discount-eligible by the holding period as at `tax_year_end_date`, and folds it into the year's gain buckets; new informational `cgt_event_e10_gain` response field. See `docs/ato/amit-cost-base-adjustments.md`
- [x] Tests: E10 excess becomes a capital gain (non-discount + discount-eligible), accumulates across years and fires only once the cost base is exhausted, and cost base floors at nil (`net_capital_gain::tests::db_e10_excess_reduction_becomes_capital_gain`, `db_e10_gain_discount_eligible_when_held_over_12_months`, `db_e10_accumulates_across_years_fires_when_cost_base_exhausted`, `portfolio::tests::db_amit_reduction_capped_at_nil_cost_base`)

## Buy-trade edit/delete integrity (symmetric with Sells)
(REQUIREMENTS "Planned Enhancements — Buy-trade edit/delete integrity". The Sell path enforces a write-time invariant; the Buy path does not.)
- [x] Reject deleting a Buy/DRP trade referenced by a parcel allocation or AMIT adjustment with a clear `422` (or `409`) instead of surfacing the SQLite FK error as `500` — `trade::db_delete` now checks (in one transaction) every table referencing trades — `parcel_allocations` (both `purchase_trade_id` and `sale_trade_id`, so a Sell deleted via `/trades` is covered too), `amit_adjustments`, and `income.reinvestment_trade_id` (a distribution's reinvestment link, same FK-500 class) — and returns `DeleteOutcome::Referenced` → `422`; the handler keeps `404` for a missing id
- [x] Reject `PUT /trades/:id` editing a Buy/DRP when the new quantity falls below the quantity already allocated out of it or covered by AMIT adjustments (`422`) — `trade::db_upsert` validates and writes in one transaction: the new quantity must be ≥ the Decimal sum of allocations out of the parcel (`UpsertError::QuantityBelowAllocated`) and ≥ every linked AMIT adjustment's covered quantity, preserving the `adjustment.quantity ≤ trade.quantity` invariant from `amit_adjustment::db_upsert` (`UpsertError::QuantityBelowAmitAdjustment`); both map to `422`, DB errors still route through `write_error_status`
- [x] Tests: delete of a consumed Buy rejected; edit shrinking a partly-sold Buy rejected; an unconsumed Buy still edits/deletes freely — `trade::tests::db_delete_buy_consumed_by_allocation_is_refused`, `db_delete_buy_covered_by_amit_adjustment_is_refused`, `db_delete_drp_linked_to_income_reinvestment_is_refused`, `db_shrink_buy_below_allocated_quantity_is_refused` (shrink-to-exactly-allocated accepted), `db_shrink_buy_below_amit_adjustment_quantity_is_refused` (shrink-to-exactly-covered accepted), `db_unconsumed_buy_still_edits_and_deletes_freely`, plus API-level `api_delete_consumed_buy_returns_422` and `api_shrink_partly_sold_buy_returns_422`
- [x] README sync: response-code/behaviour notes on the Trades endpoints — Buy/DRP integrity paragraph in the Trades HTTP API section; the `422` row in Response codes extended

## DRP enrolment and unenrolment over time
(REQUIREMENTS "Planned Enhancements — DRP enrolment and unenrolment over time", added 2026-06-06. The current single unique enrolment row per listing — presence means "enrolled" — cannot represent a holding that starts unenrolled, enrols, unenrols, and re-enrols.)
- [x] NEEDS CLARIFICATION: which date determines a distribution's reinvestability — the ex/record date (matches registry practice) or the pay date — RESOLVED 2026-06-06: the **ex date** (DRP participation is fixed at the record date, matching registry practice and the franking-credit treatment), falling back to `date_paid` when no ex date is recorded (same fallback as the 45-day rule)
- [x] NEEDS CLARIFICATION: what happens to a carried-forward residual at unenrolment — paid out when the enrolment ends, or left dormant and picked up by the first reinvestment after re-enrolment — RESOLVED 2026-06-06: **paid out** at unenrolment (the registry refunds the plan balance at termination); the carried-forward chain never crosses an enrolment period boundary
- [x] Model enrolment as dated periods per listing: enrolment date + optional unenrolment date (open-ended = currently enrolled), Residual Handling per period (a re-enrolment may choose differently) — migration `0008_drp_enrolment_periods.sql` rebuilds `drp_enrolments` via the rename pattern (id PK, listing FK, `enrolment_date` inclusive, `unenrolment_date` exclusive/nullable, per-period `residual_handling`; DROP TABLE only on the renamed `_old` table); existing rows migrate to an open-ended period starting `0001-01-01` (enrolled "since forever", preserving the old presence-means-enrolled behaviour) with their Residual Handling intact
- [x] Write-time invariants, validated atomically in a transaction: periods for a listing must not overlap, and at most one may be open at a time — `drp_enrolment::db_upsert` runs a half-open `[enrolment_date, unenrolment_date)` interval-overlap check (open end = unbounded, so a second open period always overlaps; touching periods allowed) plus an end-after-start check inside the upsert transaction; `Overlap`/`EmptyPeriod` map to `422`
- [x] CRUD API endpoints for enrolment periods (entity-module pattern), replacing/extending the current keyed-by-listing `/drp_enrolments` API — `/drp_enrolments(/:id)` now keys by period id (GET list/one, PUT upsert, DELETE), body carries `listing_id` + dates + `residual_handling`
- [x] Reinvestment checks enrolment as at the relevant date (per the clarification above): a distribution dated before enrolment or in a gap between unenrolment and re-enrolment is rejected (`422`); one dated inside a period reinvests using that period's Residual Handling — `db_reinvest` resolves the entitlement date (`ex_date` else `date_paid`), looks up the covering period in the transaction, and rejects `NotEnrolled` → `422` when none matches
- [x] Apply the residual-at-unenrolment decision (per the clarification above) to the carried-forward chain in the reinvestment operation — closing a period (`drp_enrolment::db_upsert`) moves the latest in-period DRP trade's `residual_carried_forward` to `residual_paid_out` in the same transaction (idempotent), and `db_reinvest` scopes the residual-brought-forward lookup to DRP trades within the matched period, so the chain never crosses periods
- [x] Web UI: update the DRP enrolment `ENTITIES` config for the period model (enrol/unenrol/re-enrol from the SPA) — period fields (listing, enrolment/unenrolment dates, residual handling) in the `drp_enrolments` entry; `web::tests::drp_enrolment_ui_present` asserts the period fields ship in the bundle
- [x] Tests: distribution before enrolment rejected; distribution in an unenrolment gap rejected; distribution inside a period reinvests; re-enrolment after unenrolment works; overlapping or doubly-open periods rejected with `422`; existing enrolments migrate to open-ended periods — `drp_reinvestment::tests::distribution_before_enrolment_is_rejected`, `distribution_in_unenrolment_gap_is_rejected`, `re_enrolment_after_unenrolment_uses_the_new_periods_handling`, `reinvestability_is_decided_by_ex_date_not_pay_date`, `carried_residual_does_not_cross_an_unenrolment`; `drp_enrolment::tests::db_overlapping_periods_rejected`, `db_two_open_periods_rejected`, `db_empty_period_rejected`, `db_re_enrolment_after_unenrolment_allowed`, `db_unenrolment_pays_out_trailing_carried_residual`, `db_unenrolment_only_settles_trades_inside_the_period`, `migration_converts_old_enrolments_to_open_ended_periods`, `api_put_overlapping_period_returns_422`, plus CRUD/FK/enum API tests; `ato_examples::drp_example_natalie_reinvested_dividend` re-drives the period API end-to-end
- [x] README sync: `drp_enrolments` period schema (+ Relationships), enrolment-period endpoints, reinvestment behaviour/response codes — period schema block + write-time invariant note, DRP enrolments API section (period semantics, overlap/payout behaviour, 422 cases), DRP reinvestment section (ex-date check, period-scoped residual chain), Features bullet, `residual_paid_out` column note, and the `422` Response codes row extended

## GST-inclusive brokerage entry and statement-total cross-check
(REQUIREMENTS "New Requirements — GST-inclusive brokerage entry and statement-total cross-check", added 2026-06-07. Today `trades.brokerage` is stored ex-GST and `gst_on_brokerage` is entered manually, with cost base = price×qty + brokerage + gst everywhere; broker statements quote brokerage GST-inclusive plus a net transaction total. The flag makes the server do the ÷11 split at write time — stored values keep their existing semantics, so no report changes — and the optional statement total is a pure write-time cross-check against the contract note.)
- [x] Migration: `trades` gains `brokerage_includes_gst` (INTEGER NOT NULL DEFAULT 0, CHECK in (0,1) — boolean) and `statement_total` (TEXT decimal, nullable, in the brokerage currency; informational/validation-only — no calculation uses it). Plain `ALTER TABLE ADD COLUMN` (constant defaults, no FK) — no rebuild, no data dropped; existing rows get flag 0 / total NULL — migration `0020_gst_inclusive_and_statement_total.sql`; CHECK enforced by `trade::tests::db_brokerage_includes_gst_check_constraint_enforced`
- [x] GST-inclusive split at write time, shared by `PUT /trades/:id` (Buys/DRP) and `PUT /sells/:id` (one helper — both write the `trades` table): when `brokerage_includes_gst` is true the entered brokerage amount is GST-inclusive — `gst_on_brokerage` = amount × 1/11 rounded to the cent (half away from zero, matching statements), stored `brokerage` = amount − GST, so the pair still sums exactly to the amount paid and the existing cost-base arithmetic (`brokerage + gst_on_brokerage`) is untouched. Any `gst_on_brokerage` supplied in the input is ignored when the flag is set; flag false keeps today's behaviour (ex-GST brokerage, manual GST). The flag persists so a trade round-trips (read back: split values + flag) — `trade::split_gst_inclusive`/`resolve_brokerage`, applied at the API boundary in the trade upsert handler and inside `sell::upsert_sell_in_tx` (the operations pass flag false → identity); tests `split_gst_inclusive_rounds_to_the_cent_and_sums_back_exactly`, `api_gst_inclusive_brokerage_is_split_and_round_trips`, `sell::tests::db_sell_statement_total_checks_net_proceeds_and_gst_splits`, report-level `portfolio::tests::db_cost_base_of_gst_inclusive_buy_equals_amount_paid`
- [x] Statement-total cross-check at write time inside the write transaction (both PUT paths): when `statement_total` is provided it must numerically equal (1234.50 matches 1234.5) quantity × price + brokerage + GST for a Buy/DRP, or quantity × price − brokerage − GST for a Sell (net payable/receivable); mismatch → 422 with the computed figure in the detail. A total may only be supplied when the trade currency equals the brokerage currency — supplying one on a mixed-currency trade → 422 (no FX conversion invented). Omitted total = no check (existing clients unchanged) — `trade::check_statement_total` (+ `StatementTotalError`, `statement_total_detail` for the 422 body), called from `trade::db_upsert` and `sell::upsert_sell_in_tx`; tests `api_statement_total_cross_check_on_buy` (accept, trailing zeros, mismatch detail, nothing persisted), `api_statement_total_on_mixed_currency_trade_returns_422` (superseded 2026-08-15 by SCENARIOS B-02: a mixed-currency trade is now refused outright at write time, so the total's own currency guard is unreachable — the test became `api_brokerage_in_another_currency_than_the_trade_returns_422`), `sell::tests::db_sell_statement_total_checks_net_proceeds_and_gst_splits` (subtraction direction), `api_sell_statement_total_mismatch_returns_422_with_detail`
- [x] Operation-created trades (DRP reinvestment, rights exercise, buy-back participation, scrip exchange, demerger, transfer) are unaffected: their internal trade inserts write flag 0 / total NULL (the columns' defaults; the operations' SellBody constructions pass flag false / total None) — test `drp_reinvestment::tests::reinvestment_trade_is_not_gst_flagged_and_has_no_statement_total`
- [x] Web UI: the Buy (trades) and Sell forms gain the GST-included checkbox — when ticked the GST field is hidden and the brokerage field is labelled GST-inclusive (the form re-presents brokerage + GST as one inclusive amount when editing a flagged trade) — and the optional statement-total field; the trades and Sells lists show the statement total for eyeballing against statements — shared `wireGstBrokerage` helper (attached to the trades entity config via the new generic `wireForm` hook on `viewEntityForm`, and called directly by `viewSellForm`), with `addDecimalStrings` doing the edit-form recombination as exact decimal-string addition (BigInt — no float drift on money); test `web::tests::gst_inclusive_brokerage_and_statement_total_ui_present`
- [x] Tests: flagged Buy splits the entered amount (gst = ÷11 rounded to the cent, brokerage = remainder, sum exact) and ignores a supplied GST value; flag round-trips and an edit re-splits; unflagged behaviour unchanged; cost base from a flagged trade equals the inclusive amount paid (report-level); matching statement total accepted on a Buy and on a Sell (subtraction direction); mismatched total → 422; total on a mixed-currency trade → 422; numeric-equality comparison (trailing zeros); omitted total skips the check (every pre-existing test posts no total and passes unchanged); operation-created trades carry flag 0 / NULL total; UI bundle asserts the checkbox, inclusive labelling, statement-total field, and list columns ship — all cited against the items above (10 new tests; suite 649 passing)
- [x] Docs sync: `docs/SCHEMA.md` trades block (both columns, the informational-only note on `statement_total`); `docs/API.md` Trades + Sells sections (flag semantics, the split, total validation + its 422 cases, Response codes row); README Features bullet — SCHEMA trades rows for both columns + the ex-GST note on `brokerage`; API.md Trades GST-inclusive + statement-total paragraphs, Sells example body + behaviour note (net proceeds), 422 Response codes row extended; README "Statement-friendly trade entry" Features bullet

## Simpler income entry, per-share cross-check, and combined income + DRP form
(REQUIREMENTS "New Requirements — Simpler income entry, per-share cross-check, and combined income + DRP form", added 2026-06-07, motivated by entering real registry statements: a Computershare payment advice prints amount-per-security × securities-held = gross, with the gross 100% franked for a typical company dividend — PLS FY2023 final: 0.14 × 19,695 = $2,757.30 franked, credit $1,181.70 — and a DRP advice carries the reinvestment on the same statement — VDHG 2020-10: $778.46 at $52.0017 → 14 units = $728.02, residual $50.44 carried. The income API's component model is unchanged; the server work is the two optional cross-check columns + their 422 validations, everything else is web-UI.)
- [x] Migration: `income` gains `amount_per_security` and `securities_held` (TEXT decimals, nullable, default NULL; informational/validation-only — no report or calculation uses them, mirroring `trades.statement_total`). Plain `ALTER TABLE ADD COLUMN` (constant defaults, no FK) — no rebuild, no data dropped; existing rows get NULL. The columns round-trip through the `Income` model / `FromRow` (parse failures propagate as `Decode` via the new `opt_dec` helper, never a silent zero) and `IncomeBody` — migration `0021_income_per_share_cross_check.sql`; round-trip test `income::tests::api_per_share_decimal_precision_round_trips`
- [x] Per-share cross-check at write time inside the write transaction on `PUT /income/:id`: the two fields must be supplied together — exactly one present → 422; when both present, amount_per_security × securities_held rounded to the cent (half away from zero, matching statements) must equal the gross cash components `franked_amount + unfranked_amount + foreign_source_income` (franking credits are notional, TFN withholding is deducted from the gross, both excluded) — mismatch → 422 with the computed product in the detail; both omitted = no check (existing clients unchanged) — `income::check_per_share` (+ `PerShareError`, `per_share_detail` for the 422 body) called at the top of `db_upsert`; the income upsert handler now returns `(StatusCode, String)` errors like the trades statement-total path
- [x] Tests: both example statements reconcile (`api_per_share_figures_reconcile_fully_franked_dividend` — PLS 0.14 × 19,695 = 2,757.30 fully franked; `api_per_share_product_is_cent_rounded_before_comparison` — VDHG 0.89891492 × 866 = 778.4603… vs the statement's cent-rounded 778.46); mismatched product → 422 citing the computed figure with nothing persisted (`api_per_share_mismatch_returns_422_with_detail_and_persists_nothing`); one field without the other → 422 (`api_per_share_field_supplied_alone_returns_422`); the gross includes foreign income but not credits/withholding (`api_per_share_gross_includes_foreign_income_not_credits_or_withholding`); omitted pair skips the check (`api_omitted_per_share_pair_skips_the_check`); decimal precision round-trips through the API (`api_per_share_decimal_precision_round_trips`). Also verified live against the running server (PLS/VDHG 204s, both 422 cases with their detail bodies) 2026-06-07
- [x] Web UI — simple-first income form (UI only; submits the existing component body): the form opens in a simple mode showing listing, date paid, the payment amount, the per-share pair (with the computed product shown as a live hint flagging a mismatch against the gross entered), and a franking selector **Fully franked (30%)** / **Unfranked** / **Trust distribution** — fully franked submits the amount as `franked_amount` with `franking_credits` auto-computed at amount × 30/70 rounded to the cent via exact BigInt decimal-string arithmetic (`frankingCreditFor` / `mulToCents` / `decEq` beside the existing `addDecimalStrings`; the PLS 2757.30 → 1181.70 figure shows in the hint before saving); unfranked → `unfranked_amount`; trust → `unfranked_amount` + `trust_income`. The advanced toggle reveals the full existing field set (`INCOME_ADVANCED_FIELDS`, hidden via CSS so their stored/default values still submit unchanged); editing opens in advanced mode whenever the row isn't losslessly representable simple (`incomeSimpleShape`: any advanced-only field off its default, a partially franked split, or credits ≠ the derived 30/70 figure), otherwise simple with the selector reflecting the stored shape — `wireIncomeEntry` on the income entity config, via `viewEntityForm`'s `wireForm` hook extended generically with submit-time `transformBody`/`afterSave` extensions. Verified in real Chrome (puppeteer-core driving the live server) 2026-06-07: simple-mode PLS create maps franked 2757.30 / credits 1181.70 / trust false with the per-share pair stored; the simple-eligible edit opens simple and the ex-date row opens advanced
- [x] Web UI — combined income + DRP entry (chains the two existing calls, no new endpoint): when the income has no `reinvestment_trade_id`, the form offers a "Reinvested under DRP" tick revealing the Reinvest action's fields (reinvestment price, optional trade date defaulting to the pay date, FX rate); submit does `PUT /income/{id}` then `POST /income/{id}/reinvest` via the `afterSave` hook — a reinvest failure (e.g. not DRP-enrolled) leaves the saved income standing, toasts the error pointing at the row's existing Reinvest action as the fallback. Reinvestment semantics unchanged (whole units + residual computed server-side; the VDHG advice reproduces on a fresh DB: $778.46 at $52.0017 → 14 units, residual carried 50.4362 = the statement's cent-rounded $50.44). Browser-verified 2026-06-07: a ticked save chains the reinvest and links the DRP trade (a later distribution correctly picked up the carried residual → 15 units), an unticked save doesn't reinvest, and a not-enrolled listing saves the income and shows the failure toast
- [x] Tests (web, no-browser-harness convention): `web::tests::income_simple_entry_ui_present` asserts the simple/advanced toggle, the franking selector with the 30/70 BigInt computation, the per-share product hint, the DRP tick with its chained `'/income/' + id + '/reinvest'` POST and failure-fallback wording, and the generic `transformBody`/`afterSave` hooks all ship in the bundle; existing income/holding-account UI assertions still pass (suite 658 passing, build + tests warning-free)
- [x] Docs sync: `docs/SCHEMA.md` income block (both columns + the informational-only note); `docs/API.md` Income section (the supplied-together rule + the product cross-check paragraph), web-frontend paragraph (simple-first form, per-share hint, DRP tick), Response codes 422 row extended with both per-share cases; README "Statement-friendly income entry" Features bullet; `docs/API.md` Known limitations — franking credits are auto-computed at the 30% corporate rate only (25% base-rate-entity and partially franked dividends use the advanced fields), and statement figures are keyed in manually (no statement parsing/import)

## Employee share scheme (ESS) income
(REQUIREMENTS "Employee share scheme (ESS) income" 2026-06-08. The CGT side is already correct — an RSU vest is entered as a Buy at market value at the deferred taxing point with the vest date as the acquisition date (the ATO cost-base reset). The gap is the **income** side: the assessable ESS discount must be declared in the year of the taxing point and surfaced in the tax summary, linked to the cost-base-reset Buy. **Supersedes the existing "ESS income reporting is out of scope" Known limitation** (DONE.md / README). Item 12 / ESS-statement labels.)
- [x] Mirror the ATO ESS guidance into `docs/ato/` ("tax-deferred schemes", "taxed-upfront $1,000 reduction" + its income test, "ESS and capital gains tax", the Item 12 instructions; source URL + retrieval date), indexed in `docs/ato/OVERVIEW.md` — `docs/ato/employee-share-schemes.md` (Item 12 2025 QC 104101, tax-deferred schemes, taxed-upfront $1,000 reduction QC 47628, retrieved 2026-06-08, fetched via `scripts/ato-fetch.py`); OVERVIEW.md indexes it in the "Other income components" table and the open-TODO map. The Matt worked example is mirrored and cited
- [x] New ESS-income entity + migration capturing one ESS statement, attributed to `listing_id` + `holding_account_id`: `taxed_upfront_eligible` (label D), `taxed_upfront_not_eligible` (E), `deferral_discount` (F, the RSU case), `pre_2009_cessation_discount` (G), `foreign_source_discount` (B), `tfn_withholding` (C), the taxing-point date, and the market value at the taxing point. All amounts TEXT Decimal; `currency` FK→currencies. CRUD per the entity module pattern — migration `0022_ess_statements.sql` (`ess_statements` table + nullable `trades.ess_statement_id` FK via plain ADD COLUMN); `src/entities/ess_statement.rs` is the CRUD entity. `quantity` + `market_value_per_share` were added (the per-share market value drives the vest Buy's price/cost base). `foreign_source_discount` is documented informational-only (a memo within the discount labels, surfaced by the tax summary)
- [x] Apportionment: store the **deductible/assessable amount** (post-apportionment) as the totalled value — the discount labels are entered as the ESS statement prints them; the tool does not re-apportion. The $1,000 reduction (taxed-upfront eligible): reduce the assessable discount by min($1,000, `taxed_upfront_eligible`) and surface the applied reduction (`ess_taxed_upfront_reduction`); the ≤$180,000 adjusted-taxable-income test is outside the data model, so apply the de-minimis and flag the income-test caveat as the user's responsibility — mirrors the FITO $1,000 cap pattern (`tax_summary::ess_reduction_cap_aud`, applied per year on the summed eligible discount; the caveat is documented on the field and in the API/README)
- [x] Tax summary: an **assessable ESS discount** total per Australian financial year (sum of the labels net of the applied reduction), reported separately from dividend/trust income, in AUD (foreign-source discounts converted via the ATO rate, fail-loudly with no rate), with the ESS TFN-withholding carried in the existing TFN line. CSV export carries the new fields — `ess_discount_assessable`, `ess_taxed_upfront_reduction`, `ess_foreign_source_discount` added to `TaxYearSummary`, `zero_summary`, and `CSV_HEADER`; ESS TFN folds into `tfn_withholding_tax`
- [x] ESS vesting operation (`POST /ess_statements/:id/vest`, buy-back/scrip participation pattern) ties both sides atomically: from one entry record the ESS-income discount components **and** create the cost-base-reset Buy parcel (quantity vested, price = market value at taxing point, zero brokerage, acquisition/settlement = the taxing point), linked by provenance (`trades.ess_statement_id`). Editing/deleting symmetric: the statement is frozen while vested (`PUT` → 422), and deleting the ESS record removes its linked vest Buy unless already drawn on by a Sell/allocation or AMIT adjustment (existing group-integrity rules; the vest Buy is immutable via `PUT /trades` and never deleted individually) — `src/entities/ess_vest.rs`
- [x] Web UI: CRUD + vesting operation via `ENTITIES`/`ACTIONS` config; new tax-summary columns surface automatically (derived from response keys); asserted in the served bundle — `ess_statements` ENTITIES entry with a `Vest` rowAction, an `ess-vest` confirm-only ACTIONS entry, the discount columns classified in `COLUMN_KINDS`; `web::tests::ess_statement_ui_present`
- [x] Tests: entity CRUD with decimal precision; the $1,000 reduction caps at the eligible discount and surfaces the caveat; the tax summary totals the assessable discount net of reduction in AUD (foreign-source converted, fail-loudly with no rate) and carries the TFN line; the vesting operation creates the linked Buy at taxing-point market value/date and the discount in one transaction (rolled back on failure); delete removes the linked Buy unless drawn on; web bundle assertion; an `ato_examples.rs` acceptance test (Matt, taxed-upfront eligible, QC 47628 — $2,400 discount − $1,000 = $1,400 assessable, $3,600 cost base) — `ess_statement`/`ess_vest`/`tax_summary` inline tests + `ato_examples::ess_example_matt_taxed_upfront_eligible_reduction`
- [x] Docs: `docs/SCHEMA.md` (new `ess_statements` table + `trades.ess_statement_id` + Relationships + currencies FK list), `docs/API.md` (the ESS statements endpoints/vesting operation, the new tax-summary fields, 422 causes, Web-frontend overview), and **removed the superseded ESS-income Known limitation** from `docs/API.md` (reworded to the residual limits: unvested grants, the income-test) + added the ESS-income feature to the README Features list + the tax-summary bullet

## Deductible investment expenses
(REQUIREMENTS "Deductible investment expenses" 2026-06-08. The tax summary reports gross assessable income with no deductions side, overstating the net position. Add a place to record investment-expense deductions — chiefly interest on money borrowed to buy income-producing shares, plus management/adviser fees, account-keeping fees, subscriptions — and net them in the tax summary. Distinct from the existing LIC capital gain deduction. Not present anywhere in DONE.md.)
- [x] Mirror the ATO guidance into `docs/ato/` ("Interest, dividend and other investment income deductions" + "Dividend income deductions"; source URL + retrieval date), indexed in `docs/ato/OVERVIEW.md` — `docs/ato/investment-income-deductions.md` (QC 72187) and `docs/ato/dividend-income-deductions.md` (myTax 2025 Dividend deductions, QC 104207), both retrieved 2026-06-08; indexed in OVERVIEW.md's "Other income components" table and "How this maps to open TODO items"
- [x] New `investment_expenses` entity + migration: id, date incurred, expense-type enum (`LoanInterest`/`ManagementFee`/`AdviceFee`/`AccountKeepingFee`/`Subscription`/`Other`, CHECK-constrained), amount (TEXT Decimal), `currency` (FK→currencies, AUD default), description, optional `listing_id` + `holding_account_id` FKs (both nullable — portfolio-wide expense). CRUD per the entity module pattern — migration `0024_investment_expenses.sql`; `entities::investment_expense` (typed `ExpenseType` enum via `sqlx::Type`, CHECK-enforced in the table); registered with one `pub mod` + one `.merge` in `entities/mod.rs`. No staleness triggers needed (the tax summary is not snapshotted)
- [x] Apportionment: store the **deductible amount** (post-apportionment, the figure that goes on the return) as the totalled value; optionally keep gross + deductible-percentage for provenance (informational). The tool does not rule on correct apportionment — the user's determination — `amount` is the deductible (totalled) value; `gross_amount` + `deductible_percentage` are nullable provenance, commented informational-only (no calculation reads them), per the docs' note that apportionment is the user's call
- [x] Tax summary deductions side per Australian financial year: total by expense type + overall, and a **net assessable investment income** field (existing gross totals − deductions), gross figures retained. Non-AUD expenses converted to AUD via the ATO rate at the month incurred (`infra::fx::to_aud`), fail loudly when no rate (never mix currencies / never silent zero). Tax-return CSV export carries the new columns — `TaxYearSummary` gains `gross_assessable_investment_income` (dividends_assessable + foreign_source_income + the six AMMA income components), the six per-type `deductions_*` lines, `deductions_total`, and `net_assessable_investment_income` (gross − total); expenses aggregated by `date_incurred` (July = next FY) via the shared `aud_field` (fails loudly with no ATO rate); CSV_HEADER carries the new columns in declaration order
- [x] Web UI: CRUD screen via the `ENTITIES` config; new tax-summary columns surface automatically (report columns derive from response keys); asserted in the served bundle — `Investment Expenses` ENTITIES entry (expense-type `sel`, deductible-amount + provenance `dec`s, optional listing/account `fk`s); new columns classified in `COLUMN_KINDS` (money for the amount/deduction lines, rate for `deductible_percentage`); `web::tests::investment_expenses_ui_present`
- [x] Tests: entity CRUD round-trip with decimal precision; enum/FK constraints (422 on unknown currency/listing/account); a non-AUD expense converts to AUD (fails loudly with no rate); the tax summary nets deductions by type and overall and computes net assessable income; CSV export columns; web bundle assertion — 11 inline tests in `entities::investment_expense` (decimal+provenance round-trip, optional links, delete, FK/enum 422s); 7 new in `reports::tax_summary` (net gross−deductions, gross spans income+AMMA excluding NANE/CGT, per-type totals, FY attribution, non-AUD conversion + fail-loud, CSV header); the web bundle test
- [x] Docs: `docs/SCHEMA.md` (new table + Relationships), `docs/API.md` (endpoints, new tax-summary fields, 422 causes), README Features list (investment-expense deductions) — SCHEMA.md `investment_expenses` table + Relationships (listings/holding_accounts/currencies) + currencies-FK paragraph; API.md Investment expenses section (endpoints, fields, 422 causes) + the tax-summary "Investment-expense deductions" paragraph + intro CRUD-screen list; README "Deductible investment expenses" feature bullet + the updated Tax summary bullet

## Interest income (2026-06-10)

(REQUIREMENTS 2026-06-10. The `income` entity is listing-keyed, so interest needs its own entity.)

- [x] New entity `interest_income` (standard module pattern + migration): date paid, amount, currency (AUD default; ATO-rate conversion at the month paid), TFN withholding, optional `holding_account_id`, source description — `entities::interest_income` (migration `0008_interest_income.sql`): `date_paid` (sets the FY and the ATO FX month), `amount` (the gross interest **including** any TFN amount withheld — the return's 10L convention), `tfn_withholding_tax` (10M), `currency` (FK→currencies, default AUD), free-text `source` and optional `holding_account_id` (both informational-only, commented as such). No snapshot-staleness triggers: the only reader is the tax summary, which is not snapshotted (noted in the migration, like 0006)
- [x] Tax summary: `interest_income` line per FY, included in `gross_assessable_investment_income` (and so netted by deductions); TFN amount joins the existing withholding line; CSV export updated — new `TaxYearSummary::interest_income` (read in the same snapshot transaction, bucketed by `tax_year_for(date_paid)`, AUD via the month-paid ATO rate, fail-loudly on a missing rate); gross assessable now sums it (so `net_assessable_investment_income` nets it against deductions); interest TFN joins `tfn_withholding_tax`. CSV: `interest_income` column at ATO label `10L`, and the TFN column's label extended to `10M / 11V / 13R / 12C` (docs/ato/tax-return-labels-2026.md question-10 note updated from "planned" to exported)
- [x] Web UI: `ENTITIES` entry — `interest_income` config entry (Activity group; gross-amount hint states the include-TFN-withheld convention); `interest_income` classified as a money column in `util.js` so the tax-summary report formats it
- [x] Tests: entity CRUD; FY aggregation + FX conversion + fail-loudly; gross/net identity — `entities::interest_income::tests` (decimal-precision round-trip, optional account, 404s, unknown currency/account 422); `tax_summary::tests::{db_interest_aggregated_by_financial_year, db_interest_included_in_gross_and_net_assessable, db_interest_tfn_withholding_joins_the_withholding_line, db_non_aud_interest_converted_to_aud, db_non_aud_interest_without_rate_fails_loudly, db_csv_header_carries_interest_column}` + the label assertions in `db_ato_labels_align_with_their_columns`; `web::tests::interest_income_ui_present`
- [x] Docs: `docs/SCHEMA.md` (incl. Relationships), `docs/API.md`, README Features — SCHEMA table block + Relationships (holding_accounts/currencies edges, the no-trigger exception note); API.md Interest income section, tax-summary interest paragraph, gross definition, and label-mapping rows; README Interest income feature bullet + interest named in the Tax summary bullet

## Fractional-share DRP reinvestment (2026-06-12)

(REQUIREMENTS 2026-06-12: Morgan Stanley reinvests ICE dividends in fractional shares (0.500, 0.434, …) with no residual; the whole-share-only reinvest forced nine plain-Buy workarounds priced net-cash ÷ units.)

- [x] Reinvest accepts the statement's fractional allotment — explicit `units` (broker figure authoritative, price cross-checked against reinvestable cash) or a per-enrolment whole/fractional mode; the stated units must be representable exactly — implemented as an optional `units` on `POST /income/:id/reinvest` (no schema change): the stated figure is taken exactly as the trade quantity (stored as stated, trailing zeros included), `units × price` is cross-checked against the available cash (reinvestable cash + residual brought forward) to within **one unit-step at the units' stated precision** (the property any broker-computed allotment has whatever its rounding direction; a full step or more off rejects with `422` carrying both figures), and the residual columns record zero (the sub-step difference is statement rounding, not cash). Tests: `drp_reinvestment::tests::explicit_units_take_the_statements_fractional_allotment` (exact storage incl. scale), `explicit_units_tolerate_sub_step_statement_rounding`, `explicit_units_cash_mismatch_is_rejected` (incl. the exclusive boundary), `non_positive_units_are_rejected`, `explicit_units_still_require_enrolment`, `explicit_units_spend_the_brought_forward_residual`, `api_reinvest_with_units_returns_201_with_fractional_trade`, `api_reinvest_units_mismatch_returns_422_with_figures`
- [x] Whole-share floor + residual carry stays the default; all existing whole-share tests unchanged — `units` omitted keeps the floor + residual-handling path verbatim; every pre-existing `drp_reinvestment` test passes unmodified
- [x] Live-data check: the nine ICE plain-Buy reinvestments are re-enterable through the reinvest operation with the statements' exact fractional units — pinned by `drp_reinvestment::tests::morgan_stanley_ice_fractional_statements_reproduce` (the nine statements' gross/withholding/units/price figures through `db_reinvest`, exact stated units stored), and applied to the live DB 2026-06-12: trades 31–39 (the plain-Buy workarounds; unreferenced by allocations/AMIT/attachments) deleted and income 37–45 reinvested via the API with the statements' exact units against the existing ICE/Morgan-Stanley enrolment (id 3) — new DRP trades 9018–9026 carry identical date/quantity/price/account with the income link restored and zero residuals; total ICE units in the account unchanged (286.919). Pre-change backup: `/tmp/share-tracker-pre-fractional-reentry-backup.db`. The live run is not reproducible in-repo (the archive DB isn't committed)
- [x] Docs sync: `docs/API.md` DRP reinvestment section, `docs/SCHEMA.md` if a column is added, README DRP feature bullet, web UI reinvest form — API.md gains the "Fractional allotments (`units`)" paragraph (example body, tolerance rule, `422`s) and the Response-codes `422` row names the units rejections; README's DRP bullet covers fractional broker plans; the web UI adds the optional "Units allotted (fractional plans)" field to both the Reinvest action form (`config.js`) and the income form's chained DRP section (`forms.js`, omitted from the body when blank); `docs/SCHEMA.md` unchanged — no column was added. Pinned by `web::tests::income_simple_entry_ui_present` and `web::tests::post_actions_are_config_driven`

## ESS statement AUD override (2026-06-12)

(REQUIREMENTS 2026-06-12: employer statements convert at release-date spot, the tax summary at the RBA monthly rate — $65–214/yr apart in the live data; the ATO prefill carries the employer's AUD figure.)

- [x] `ess_statements` gains optional statement-AUD discount amounts (at minimum the total assessable discount); tax summary reports them verbatim when present, RBA-converts as today when absent — implemented per label: migration `0009_ess_statement_aud_overrides.sql` adds nullable `aud_taxed_upfront_eligible`/`aud_taxed_upfront_not_eligible`/`aud_deferral_discount`/`aud_pre_2009_cessation_discount`/`aud_foreign_source_discount` (mirroring the employer statement, which states each label in AUD at the release-date spot rate); the tax summary's `aud_label` helper reports a present override verbatim (including the $1,000 taxed-upfront-reduction input) and falls back to the RBA monthly conversion otherwise. An override on an AUD-denominated statement is rejected `422` (two AUD figures for one label could silently disagree). The vested-statement freeze is relaxed to the vest-Buy-driving fields only (listing, account, taxing point, quantity, market value, currency) so the income side — discount labels, TFN withheld, the new overrides — stays enterable after vesting, which is when the employer's annual statement actually arrives. Tests: `ess_statement::tests::db_round_trips_statement_aud_overrides`, `db_aud_override_on_aud_statement_rejected`, `api_aud_override_on_aud_statement_rejected_422`; `tax_summary::tests::db_ess_statement_aud_override_reported_verbatim`, `db_ess_aud_override_drives_the_taxed_upfront_reduction`; `ess_vest::tests::deleting_the_statement_removes_the_vest_buy` (vest-side edit still 422, income-side edit passes)
- [x] Live-data check: with the employer AUD figures entered, `ess_discount_assessable` equals the ATO ESS statements exactly (FY2022 10,572; FY2023 9,443; FY2024 11,731; FY2025 13,526) — pinned by `tax_summary::tests::db_ess_aud_overrides_reproduce_the_employer_ess_statements` (the five real releases with the four annual-statement AUD figures; FY2026 has no annual statement yet and keeps the RBA conversion), and applied to the live DB 2026-06-12: `aud_deferral_discount` set on statements 1–4 via the API (the figures from the archive's `NNNN06 ESS` annual statements, label F), live `GET /portfolio/tax-summary` verified to report 10,572 / 9,443 / 11,731 / 13,526 exactly with FY2026 still RBA-converted. Pre-change backup: `/tmp/share-tracker-pre-ess-aud-backup.db`
- [x] Docs sync: `docs/SCHEMA.md` ess_statements block, `docs/API.md` ESS section + Tax summary, web UI ESS form fields — SCHEMA gains the five override columns + the relaxed-freeze note; API.md ESS statements gains the "Statement-AUD overrides" paragraph and the rewritten vested-edit rule, the Tax summary ESS paragraph documents verbatim-when-present, and the Response-codes `422` row names both new rejections; README's ESS feature bullet covers the override + post-vest editability; the web ESS form gains the five optional "Statement AUD" fields (pinned by `web::tests::ess_statement_ui_present`)

## statement_total tolerance for cent-rounded contract notes (2026-06-12)

(REQUIREMENTS 2026-06-12: contract notes print the consideration cent-rounded; 3 of 41 archive notes were rejected by the exact comparison and entered without the cross-check.)

- [x] The cross-check passes when the supplied total equals the computed figure rounded to the cent (half away from zero); exact matches keep passing; larger mismatches still 422 with the computed figure in the body (Buys and Sells) — `trade::check_statement_total` now accepts the computed figure cent-rounded via `round_dp_with_strategy(2, MidpointAwayFromZero)` (the same strategy statements use) alongside the exact match; the `TotalMismatch` detail still carries the unrounded computed figure. The shared helper covers Buys, DRPs, and Sells (both `db_upsert` paths call it). Tests: `trade::tests::api_statement_total_accepts_cent_rounded_contract_note_totals` (the three archive figures below, plus a 100.005 midpoint pinning away-from-zero over banker's rounding and a one-cent-off rejection carrying `48946.360028`), `sell::tests::db_sell_statement_total_accepts_cent_rounded_net_proceeds` (33 × 14.906273 − 9.95 = 481.957009 accepts 481.96, rejects 481.95); all pre-existing exact-match tests pass unchanged
- [x] Live-data check: trades 16, 19, 21 (the three entered without the cross-check) accept their contract-note totals — pinned by `trade::tests::api_statement_total_accepts_cent_rounded_contract_note_totals` (trade 19 HNDQ note 1404967: 1,302 × 37.585914 + 9.50 = 48,946.360028 → note 48,946.36; trade 16 VDHG note 4518597: 562 × 73.259875 + 9.50 = 41,181.54975 → note 41,181.55; trade 21 ETH 22 Sep 2021: 0.02413796 × 3,983.77 + 3.84 = 100.000080… → note 100.00), and applied to the live DB 2026-06-12: all three totals PUT through the API (HTTP 204, GST splits unchanged) and now stored. Pre-change backup: `/tmp/share-tracker-pre-stmt-total-backup.db`. The live run is not reproducible in-repo (the archive DB isn't committed)
- [x] Docs sync: `docs/API.md` Trades + Sells statement_total paragraphs, Response codes 422 row — the Trades paragraph documents the exact-or-cent-rounded acceptance (with the 48,946.360028 → 48,946.36 example), the Sells paragraph notes the tolerance applies to net proceeds too, and the 422 row reads "neither exactly nor cent-rounded"

## Lossless trade round-trip for GST-inclusive brokerage (REQUIREMENTS 2026-07-13)
Found scripting against the API during the 2026-07-13 crypto reconciliation: on a trade stored with
`brokerage_includes_gst` set, `GET /trades/:id` returns the stored ex-GST split (`brokerage` +
`gst_on_brokerage`) alongside the flag, but `PUT /trades/:id` with the flag set interprets
`brokerage` as the one GST-inclusive amount and re-splits it — so a faithful GET→edit→PUT
round-trip silently shrinks the brokerage by the GST each pass (0.99 stored → read back 0.90 +
0.09 → re-split 0.82 + 0.08), with no 422. The web form escapes only because `wireGstBrokerage`
recombines the pair before saving; every other API client hits silent data corruption.
- [x] Decide and implement the lossless shape (design-open per REQUIREMENTS): either reads present `brokerage` as the same GST-inclusive amount the write path expects when the flag is set (updating the web form in the same step so it doesn't double-recombine), or the write path accepts the stored split pair as-is when supplied intact — either way the read/write asymmetry goes — **decided 2026-07-13: reads present the inclusive amount.** `Trade::present` (applied in `GET /trades` and `GET /trades/:id` only — internal `db_get`/`db_list` callers keep the stored ex-GST split) re-presents a flagged trade's `brokerage` as the recombined inclusive amount, with `gst_on_brokerage` still carrying the derived component (informational on reads, ignored by flagged writes). So `brokerage` means the GST-inclusive amount on both reads and writes whenever the flag is set — symmetric, no split-pair heuristics on the write path. `wireGstBrokerage` in `web/forms.js` no longer recombines the pair client-side (it would double-count the GST on top of the server's re-presentation); the form fills the field straight from the row
- [x] Cover both write paths that share `resolve_brokerage` — `PUT /trades/:id` and `PUT /sells/:id` — so a flagged Sell round-trips losslessly too — a Sell reads back via the same `GET /trades/:id`, so the one presentation point covers both; `sell::tests::api_gst_inclusive_sell_get_put_round_trip_is_lossless` proves the Sell path
- [x] Regression test: PUT a GST-inclusive trade, GET it, PUT the response body back verbatim, assert the stored `brokerage`/`gst_on_brokerage` are unchanged (and the same for a flagged Sell) — `trade::tests::api_gst_inclusive_get_put_round_trip_is_lossless` (two GET→PUT-verbatim passes on the REQUIREMENTS 0.99 example, a round-tripped `statement_total`, and the unflagged read-as-stored case) and the Sell test above (likewise two passes); `web::tests::gst_inclusive_brokerage_and_statement_total_ui_present` pins that the form's client-side recombination is gone
- [x] Docs: `docs/API.md`'s GST-inclusive brokerage section states the round-trip semantics explicitly — Trades section states the read shape, the both-reads-and-writes meaning of `brokerage`, and the verbatim-re-PUT guarantee; Sells section carries it for flagged Sells; pinned by `doc_checks::gst_inclusive_round_trip_semantics_documented`

## Attachment coverage: more owners, plain-text files (REQUIREMENTS 2026-07-15)
Found while attaching the statement archive to the recorded activity: plain-text records and two
entity types (ESS statements, interest income) had no attachment path.
- [x] `text/plain` joins the attachment content-type allowlist (DB CHECK + `ContentType::Txt`), so `.txt` records (crypto exchange trade records, DRP advices) attach like PDFs — a charset parameter on the declared MIME type doesn't defeat the match; tests `attachment::tests::api_upload_accepts_text_plain` (upload + download round-trip carries `text/plain`), `api_upload_rejects_unsupported_content_type` / `db_content_type_enum_constraint_enforced` (repointed at `application/zip` to keep pinning the allowlist boundary)
- [x] ESS statements and interest income records own attachments like trades/income/AMMA statements: two new nullable FK owner columns (`ess_statement_id`, `interest_income_id`, ON DELETE CASCADE) in the exactly-one-owner CHECK, `POST /attachments` owner fields, `?ess_statement_id=`/`?interest_income_id=` list filters, and the web UI Attachments action on both entity screens (`attachOwner` config + `ATTACH_OWNER` naming map) — tests `attachment::tests::api_upload_to_ess_statement_and_interest_income_owners`, `deleting_new_owner_rows_cascades_to_attachments`, `web::tests::attachments_ui_present` (pins all five owner wirings + the `.txt` file-picker accept)
- [x] Migration `0014_attachment_owner_expansion.sql` rebuilds the table via the rename pattern (both rules live in table-level CHECKs SQLite can't ALTER; ids and rows copied verbatim — verified against a copy of the live DB) and re-creates the two `attachments_row_history_*` triggers with the expanded column list per the audited-table rule — pinned by `row_history::tests::audited_tables_match_migration_check_and_triggers` (extended to assert 0014 re-creates both triggers recording the new columns)
- [x] Docs: API.md Attachments section (five owners, `text/plain` in the allowlist, web-frontend Attachments-action list) and SCHEMA.md (attachments columns + CHECK, Relationships line, five-FK prose) updated in the same change

## DRP trades show the funding distribution's attachments (REQUIREMENTS 2026-07-15)
Every DRP statement in the archive is attached to the income row it was entered from (the Reinvest
action creates the DRP trade *from* that row, and the one advice documents both the distribution
and the reinvestment), so a DRP trade's own Attachments view is always empty today — the paperwork
exists but is not discoverable from the trade.
- [x] A DRP trade's Attachments view also lists the linked income row's attachments (traversing `reinvestment_trade_id`), clearly labelled as the income row's documents: download works from there, upload from the trade's view still attaches to the trade, delete stays on the owning record's view. Attachments stay single-owner — a read-time traversal (web UI or a list-endpoint option, design-open), no data-model change
- [x] The same rule for the other provenance-created trades whose source record owns attachments (an ESS vest Buy shows its `ess_statements` row's attachments; a buy-back Sell its income row's) — enumerate the provenance links at implementation time
- [x] Docs: `docs/API.md` if the list endpoint gains the linked-owner option; the Attachments feature text mentions linked documents

Implemented as a list-endpoint option (the design-open choice): `GET /attachments?trade_id=…&include_linked=true`
(`attachment::db_list_with_linked`) also returns the linked source record's attachments — the enumerated
provenance links are `income.reinvestment_trade_id` (DRP funding distribution), `income.buyback_trade_id`
(buy-back participation Sell's dividend row), and `trades.ess_statement_id` (ESS vest Buy's annual statement);
the other provenance-created trades trace to records that cannot own attachments. Ownership unchanged (rows
carry their true owner's FK); `include_linked` without a lone `trade_id` filter → 422. The web UI's trade
Attachments view passes the flag, labels linked rows ("distribution #N (linked)" via an attached-to column
shown only when a linked row exists), and offers an "Owner's attachments" link instead of Delete on linked
rows. Tests: `entities::attachment::tests::api_list_include_linked_returns_drp_funding_income_attachments` /
`…_buyback_income_attachments` / `…_ess_statement_attachments` / `…_requires_a_lone_trade_filter`,
`web::tests::attachments_trade_view_lists_linked_source_documents`, and
`doc_checks::linked_attachments_documented` (API.md documents the option, all three links, and the feature text).
Verified end-to-end: real reinvest flow on a scratch DB, advice uploaded to the income row, shown labelled on
the DRP trade's UI view with download working and no Delete; the income row's own view unchanged.


## AMIT adjustment cross-check and generation (REQUIREMENTS 2026-08-13)
Entering an AMMA statement creates nothing else: the per-parcel `amit_adjustments` rows that apply
its per-unit `cost_base_adjustment` are hand-entered afterwards (FY2025 VDHG needs 30). Each row is
validated in isolation by `amit_adjustment::db_upsert` (Buy/DRP, listing, holding account, quantity
cap) but the *set* is never checked against its statement, so a missed parcel silently overstates
cost base and a duplicated one over-reduces it — and because CGT event E10 floors at nil, an
over-reduction can manufacture a capital gain. Two halves: a cross-check report that verifies a set,
and a generation action so the set need not be typed at all. No schema change beyond the optional
UNIQUE index below.

- [x] New non-blocking report `GET /reports/amit_adjustment_cross_check`
      (`src/reports/amit_adjustment_cross_check.rs` + `pub mod` / `.merge(...)` in
      `src/reports/mod.rs`), following `reports::amit_cash_cross_check`/`e4_cross_check`: all inputs
      on one `pool.begin()` read transaction, empty result = everything reconciles. One row per
      flagged AMMA statement carrying `amma_statement_id`, `listing_id`, `ticker`, `tax_year`
      (via `domain::tax_year::tax_year_for`), `holding_account_id`, `units_held`, `units_adjusted`,
      `parcel_count`, and the list of problems found
- [x] Check: **no adjustments at all** on a statement whose `cost_base_adjustment` is non-zero
      (highest signal — the whole statement's cost-base effect is missing). A statement with a zero
      per-unit figure is not flagged
- [x] Check: **coverage mismatch** — Σ `amit_adjustments.quantity` ≠ `amma_statements.units_held`,
      reported with the signed difference. Must re-base through the listing's splits before
      comparing (`corporate_action::adjustments::split_adjusted_quantity` / `as_acquired_quantity`):
      adjustment quantities are as-acquired units, `units_held` is the statement year's basis, so a
      naive comparison false-positives on any split
- [x] Check: **duplicate parcel** — the same (`amma_statement_id`, `trade_id`) pair more than once
- [x] Check: **parcel outside the statement's year** — the two unambiguous cases only: trade `date`
      after `tax_year_end_date`, or the parcel fully consumed by allocations whose sale trades all
      predate 1 July of that FY. A parcel disposed of *during* the year is legitimate and must not
      be flagged
- [x] Write-time: duplicate (`amma_statement_id`, `trade_id`) pairs rejected `422` from
      `amit_adjustment::db_upsert` — a new `UpsertError` variant with its arm in the existing
      `From<UpsertError> for ApiError` impl. Unlike the other checks this is a real data-model
      invariant. Verify the **deployed** DB (bigbrain.lan, not the repo copy) has no existing
      duplicate pairs before adding a UNIQUE index in a migration; the repo copy was clean as at
      2026-08-13
- [x] `POST /amma_statements/:id/generate_adjustments` — creates one `amit_adjustment` per open
      parcel as at `tax_year_end_date`, sourced from `domain::open_parcels::load(conn, as_of)` with
      each `remaining_as_of` converted back to the as-acquired basis the quantity column stores, and
      filtered to the statement's own `listing_id` + `holding_account_id`. All rows in one
      transaction (no partial set can persist), each written **through
      `amit_adjustment::db_upsert`** rather than a bulk INSERT so the per-row invariants and the
      `row_history` audit trail apply to generated rows exactly as to typed ones
- [x] Generation response echoes `created` (the rows), `units_adjusted`, `units_held` and their
      difference. A mismatch does **not** block the write — it is a reconciliation, not an invariant
      (a statement may state units at a date other than year end) — it is surfaced in the response
      and stays flagged by the cross-check report until resolved
- [x] Generation refuses `422` when: the statement already has adjustments (unless `replace: true`,
      which deletes and regenerates in the same transaction); there are no open parcels as at that
      date (a statement for a position the system doesn't have is itself the error, and an empty set
      would hide it); or the listing has a split between the earliest covered parcel's acquisition
      and `tax_year_end_date` leaving covered parcels on different unit bases — a single per-unit
      `cost_base_adjustment` cannot correctly scale both sides of a split. Pre-existing modelling
      limit (hand entry has it too, with no error message); a guard, not a blocker — neither AMIT
      listing held today has a split
- [x] Web UI: saving an AMMA statement offers generation as the next step, the same
      chain-after-save shape the income form's "Reinvested under DRP" tick uses. The confirm step
      previews the parcels and quantities it will create and shows Σ against the statement's
      `units_held`, so "are the current positions correct?" is checkable rather than assumed; a
      mismatch is shown prominently and the user can still proceed
- [x] Web UI: a standing `ACTIONS` entry in `config.js` on the AMMA statement row runs generation
      later, or re-runs it with `replace` after correcting a missed trade (the common repair path —
      a missing parcel usually means a trade was entered after the statement); plus the `REPORTS`
      entry for the cross-check under Reports → Cross-checks & alerts beside the AMIT Cash
      Cross-Check, with its numeric columns classified in `util.js`'s `COLUMN_KINDS`
- [x] Annual tax report picks the cross-check up: `reports::tax_report::Completeness` gains a
      fourth list beside `amma_missing`/`amit_cash_alerts`/`e4_alerts`, filtered to the report's
      year on the row's `tax_year` exactly as those two are, and `complete` becomes "all four
      empty". Read via the new report's own pool-based `db_*` function on its own snapshot, not
      folded into the report's main transaction — the same advisory-note reasoning the module
      header already documents for the other two (that header's "two existing cross-checks" becomes
      three). This is the answer to "verify before the annual tax statement is run": the
      completeness section is exactly that gate, and an AMIT adjustment gap distorts the disposal
      schedule's cost base, which is the report's central figure
- [x] `taxreport.js`'s `completenessSection` renders the new alerts as a fourth bullet type; the
      existing ✓/⚠ badge and its "this report may understate income or the cost base until they are
      resolved" wording already cover them. Deliberately **not** a hard gate on generating the
      report — completeness stays non-blocking (`docs/API.md`: "never rejects the request"). A
      warning printed onto the archived PDF is a stronger safeguard than a refusal, since it travels
      with the document, and the report is often generated precisely to find out what is wrong
- [x] Tests: per-check report tests (each flag fires; a correct set flags nothing; a split does not
      false-positive the coverage check; a mid-year disposal is not flagged); `db_upsert` duplicate
      rejection at DB and API level; generation reproduces the hand-entered HNDQ FY2024/FY2025 sets
      exactly (509+1302 = 1811, and the five parcels totalling 2620 with the 2025-07-16 DRP
      excluded — the empirical case the requirement is built on); each `422` refusal; the annual tax
      report's `completeness` flags an adjustment gap, drops `complete` to false, and clears once
      the adjustments are entered (mirroring the existing `amma_missing` tests at
      `tax_report.rs:1526`); the new delete route (if any) added to
      `entities::tests::deleting_a_missing_row_is_404_naming_what_was_missing`
- [x] Docs: `docs/API.md` gains the new report and the generation endpoint (request/response shapes,
      each `422`, and the Response-codes section) and updates the Annual tax report `completeness`
      bullet (currently "true only when all three are empty"); README's Features list alongside the
      other cross-checks, and its Annual tax report bullet's completeness wording;
      `docs/SCHEMA.md` only if the UNIQUE index lands

Implemented across three new pieces plus the wiring. The **cross-check report**
(`src/reports/amit_adjustment_cross_check.rs`, `GET /reports/amit_adjustment_cross_check`) reads
statements, adjustments-joined-to-parcels, the split events and every sale allocation on one
`pool.begin()` snapshot, and returns one row per non-reconciling statement carrying every problem
found as a `problems: Vec<String>` of self-contained sentences. All four checks landed as specified,
with the coverage comparison re-basing each as-acquired quantity to the statement year through
`split_adjusted_quantity` before summing (`db_a_split_does_not_false_positive_the_coverage_check`
pins both directions). **Generation** went into its own operation module
(`src/entities/amit_adjustment_generation.rs`, the `ess_vest`/`scrip_exchange` shape) rather than
growing `amma.rs`: it sources parcels from `domain::open_parcels::load(conn, Some(tax_year_end_date))`
on its own transaction, filters to the statement's listing + holding account, converts each
`remaining_as_of` back to as-acquired units, and writes every row through the new
`amit_adjustment::db_upsert_on` (the connection form of `db_upsert`, `db_tax_summary_on`'s naming) so
generated rows pass the same per-row invariants and land in `row_history` exactly as typed ones do.

Two things the requirement left open were settled in implementation. The split guard asks the
public higher-level question (`split_adjusted_quantity(ONE, …)` per covered parcel, all equal = one
unit basis) rather than the deliberately-internal raw ratio, so a split *before* every covered parcel
— which scales them all alike — generates normally and only a split *between* them refuses. And the
confirm step is a `"preview": true` flag on the generation endpoint itself (answering `200` with the
rows it would create and rolling back) rather than a second endpoint: it runs every refusal, so the
preview shows the same `422` the write would. `viewAction` gained one generic hook for it —
`action.confirm(path, body)`, an async gate run before the POST — so the preview is config-driven
like everything else in ACTIONS, not a bespoke view.

Empirical case reproduced: `db_generation_reproduces_the_hand_entered_hndq_sets` builds the live
HNDQ holding and asserts generation produces exactly the hand-entered FY2024 set (509 + 1302 = 1811)
and FY2025 set (five parcels totalling 2620, the 2025-07-16 DRP correctly excluded), then that the
cross-check reconciles both. The deployed DB (bigbrain.lan, 149 `amit_adjustments` rows) and the repo
copy were both checked for duplicate `(amma_statement_id, trade_id)` pairs on 2026-08-13 and had
none, so migration `0022_amit_adjustment_unique_parcel.sql` adds the UNIQUE index with no data step;
`UpsertError::DuplicateParcel` gives the rejection this module's own wording, with the index as the
backstop. Web UI: `wireAmmaEntry`'s chain-after-save tick (ticked by default on a new statement),
the standing `generate-adjustments` ACTIONS entry with its Replace field, the REPORTS entry under
Cross-checks & alerts, `units_adjusted` classified in `COLUMN_KINDS`, and `cellText` rendering a
list-valued cell as sentences. Both new screens verified rendering real seeded data via
`scripts/ui-check.sh`. Docs: API.md gained the generation section (each 422, the preview mode), the
cross-check section, the duplicate 422, the four-part completeness bullet and the Response-codes
entries; README the feature line and the completeness wording; SCHEMA.md the UNIQUE index — pinned by
`doc_checks::amit_adjustment_generation_and_cross_check_documented`.

## Brokerage in a currency other than the trade's is added to the cost base unconverted (SCENARIOS B-02)
(SCENARIOS.md section B verification pass, 2026-08-15. `domain::cost_base`'s `initial_cost` is
`average_price × quantity + brokerage + gst_on_brokerage`, summed in the trade's currency and
converted to AUD as one figure at the acquisition-month rate. `trades.brokerage_currency` is
FK-validated against `currencies` and then read by exactly one thing — `check_statement_total`,
which *refuses* the statement-total cross-check when it differs from `currency`. No calculation
consults it, and the field carries no informational-only comment, so the model invites an entry it
then mis-costs.)
- [x] Reproduced: USD listing, RBA USD rate 0.50 for 2024-01, Buy 10 @ USD 100 with
  `brokerage: "30"`, `gst_on_brokerage: "3"`, `brokerage_currency: "AUD"` (an Australian broker's
  AUD fee on a US trade). `/portfolio/open-parcels` reports `original_cost_base` **A$2,066**; the
  correct figure is **A$2,033** (USD 1,000 ÷ 0.50 = A$2,000, plus the A$33 already in AUD). The
  A$33 fee was converted as though it were USD
- [x] Same on the disposal side: a Sell's proceeds net the brokerage before conversion, so a
  foreign-currency fee on a foreign-currency sale is netted at the wrong scale
- [x] Not covered by any Known limitation, and no test pins a cost base with a mixed
  brokerage/trade currency (`brokerage_currency` appears in `src/` only in fixtures and the
  statement-total guard)
- [x] Decide the fix: convert the brokerage leg separately at its own currency's rate (element 2 is
  an amount actually incurred, translated at its own time per s 960-50 — `docs/ato/
  forex-common-transactions.md`), or refuse a `brokerage_currency` that differs from `currency` at
  write time the way `statement_total` already does for the same pair. Refusing is honest and
  cheap; converting is what the field promises
- [x] Tests: `domain::cost_base` / `reports::open_parcels` for whichever route, plus the Sell side
- [x] Docs sync: `docs/API.md` Trades (what `brokerage_currency` means for the cost base) and
  `docs/SCHEMA.md`

**Resolution (2026-08-15): refuse the mismatched pair at write time.** Converting each leg at its
own rate is what s 960-50 says, but the brokerage doesn't feed one figure — it feeds four, each a
single-currency sum: the Buy/DRP cost base (`domain::cost_base::Parcel::initial_cost`), a Sell's
proceeds net of costs (`reports::realised_gains`), the performance report's net trade flow
(`reports::performance`), and `TradeAmounts::net_transaction_total` (the statement-total cross-check
and the activity ledger's row amount). Threading `FxRates` and a translation month through all four
leaves any missed site silently wrong, and the ledger's amount has no single currency to be reported
in at all. One write-time invariant makes the mixed state unrepresentable instead, which is where
this project puts data-model invariants anyway. The accuracy cost is nil: converting the fee into
the trade's currency at the trade month's rate reproduces the exact AUD cost base, because the whole
figure converts at that rate downstream (A$33 → USD 16.50 at 0.50 → A$2,033).

The check lives in `trade::check_amounts` (new `AmountsError::BrokerageCurrencyMismatch`, compared
case-insensitively), which both write paths — `trade::db_upsert` and `sell::upsert_sell_in_tx` —
already run before anything is written, so the Buy and Sell sides are covered by one rule and can't
drift; `AmountsCheck` gained the `currency`/`brokerage_currency` pair it compares. Every internal
trade-creating path (ESS vest, rights exercise, DRP reinvestment, inheritance, rollover
replacements, buy-back participation) already binds the same currency to both columns, and
`test_support`'s `.currency()` setter sets the pair together, so nothing generated could produce a
mismatch. `StatementTotalError::CurrencyMismatch` became unreachable and was removed with its
detail wording and the two currency fields on `StatementTotalCheck` — a mixed-currency trade can no
longer exist for the total to be checked against.

Tests: `trade::tests::api_brokerage_in_another_currency_than_the_trade_returns_422` (with and
without a statement total, nothing persisted, and the same fee converted into the trade's currency
accepted — replacing `api_statement_total_on_mixed_currency_trade_returns_422`, whose scenario is
now refused a step earlier), `sell::tests::api_sell_brokerage_in_another_currency_than_the_sale_returns_422`
for the disposal side, and
`reports::open_parcels::tests::db_foreign_fee_recorded_in_the_trade_currency_costs_at_its_own_scale`
pinning the finding's own figures at A$2,033. `db_unknown_currency_rejected_on_both_currency_columns`
now carries the unrecognised code on both columns (the only way it can reach the database) and pins
`brokerage_currency`'s own FK with a direct `UPDATE`, since no write path can reach it alone; the
`open_parcels` USD fixture no longer forces an AUD fee onto a USD trade. Docs: a Known-limitations
entry (the rule, the entry route it leaves the user, and the s 960-50 timing it doesn't model), the
Trades section's `brokerage_currency` paragraph, the statement-total and core-figures paragraphs,
the Response-codes 422 row, `docs/SCHEMA.md`'s column note, and the README limitations line — pinned
by `doc_checks::known_limitations_document_the_brokerage_currency_invariant`. Full suite 1403 passed
/ 0 failed; `cargo build`, `cargo fmt --check`, and `cargo clippy --all-targets -D warnings` all
clean.

---

## SCENARIOS S-10: a trade may be dated in the future, and a financial year that has not happened then appears in the annual tax report's year picker

`PUT /trades/:id` and `PUT /sells/:id` accept any future date. Driven against the running system on
2026-08-22:

- Buy dated **2027-06-01** → `204`, settlement 2027-06-03.
- Buy dated **2028-03-01** → `204`; Buy dated **2028-04-13** → `204`.
- Sell dated **2027-06-01** → `204`, allocating a real parcel.
- Crypto Buy dated **2030-01-01** → `204`, settlement same day.
- `GET /reports/tax-report/years` then answers **`[1986, 2026, 2027, 2028, 2030]`**, and
  `POST /reports/tax-report {"tax_year": 2030}` is inside `TaxYear`'s accepted range, so the annual
  tax report will render a financial year that has not begun.

The rest of the system is consistent the other way: `POST /listings/:id/rename` refuses a future
`effective_date` (`RenameError::FutureDated`, closed as SCENARIOS R-02 in `59bb595`),
`PUT /closing_prices/:listing/:date` refuses one with `the close of <date> is not final yet`, and
`net_capital_gain`'s quiet-carry-forward year is deliberately bounded at `tax_year_for(today())`
(SCENARIOS O-x, `319b159`). A trade is the only dated fact with no upper bound at all.

Reports read as at today are **not** corrupted — `domain::open_parcels::load` filters on `as_of`, so
the future parcels are correctly absent from `GET /portfolio/open-parcels` and the portfolio
overview. The damage is confined to the year-keyed surfaces and to the typo going unnoticed (a
2027-for-2026 slip on a July trade is exactly the shape this catches).

**Live database: zero rows disagree** — no trade is dated after today (latest is 2026-07-16), so a
write-time refusal would leave every existing row editable.

**Decision (Evan, 2026-08-22): refuse it.** Rejected: a health alert alone, and capping the year
picker while still accepting the trade.

- [x] Refuse a `date` after the server's current date on `PUT /trades/:id` and `PUT /sells/:id`,
      via `check_amounts` (a new `FutureDate` variant beside `PreCgtDate`, its natural twin — one
      bounds the date below, the other above)
- [x] Settle the two direct-INSERT paths the way `PreCgtDate` was settled (refused on the *statement*
      in `ess_vest`, the earlier and better place): an ESS taxing point and a date of death are both
      already-happened facts, so the same argument holds, but each module-doc list needs its line
      — done: neither `ess_statement::db_upsert` nor `inheritance::db_upsert` bounded its date above,
      so each got the bound (`UpsertError::FutureTaxingPoint` / `UpsertError::DeathInFuture`) beside
      its pre-CGT twin, and both module-doc lists carry the `FutureDate` line
- [x] `settlement_date` is **not** in scope — a T+2 settlement of a trade dated today is legitimately
      in the future (pinned: the boundary tests assert the accepted trade dated *today* stores a
      settlement date after today)
- [x] `docs/API.md` 422 catalogue + the trades/sells sections; `docs/SCHEMA.md` if the column comment
      needs it — also the ESS-statement and inheritance sections, the `tax-report/years` paragraph,
      and the `trades.date` / `taxing_point_date` / `date_of_death` column comments
- [x] Regression tests: tomorrow refused on `PUT /trades/:id` and `PUT /sells/:id`, today accepted
      (the boundary), and `GET /reports/tax-report/years` never offering a year beyond
      `tax_year_for(today())`

Consequences found while implementing, all settled in the same commit:

- `check_amounts` is shared with `sell::upsert_sell_in_tx`, which every parcel-substituting
  operation writes its closing Sell through — so a scrip exchange, demerger, transfer, buy-back
  participation or worthless-shares recognise **performed before its own date** is now refused too.
  That is the right answer (its replacement parcels would be dated in the future and so absent from
  the live view), but three of those five answered a generic "the … parcel allocations are invalid"
  and `transfer`'s catch-all logged `tracing::error!` "unexpected sell rejection" — each now names
  the future date instead.
- The year picker needed its **own** bound: `db_tax_report_years` unions every dated fact, and
  interest income / AMMA / ESS / investment expenses are not date-bounded, so it filters at
  `tax_year_for(today())` rather than inheriting the trade write path's ceiling.

---

---

## SCENARIOS S-08: a trade may be dated on a day its exchange did not trade, and nothing refuses or flags it

`PUT /trades/:id` and `PUT /sells/:id` accept any date from 1985-09-20 on. Driven against the
running system:

- Buy on **Saturday 2026-05-16** (XASX) → `204`, settlement 2026-05-19.
- Buy on **Good Friday 2026-04-03** (a seeded `exchange_holidays` row for XASX) → `204`,
  settlement 2026-04-08.
- Buy on **Christmas Day 2026-12-25** (seeded) → `204`, settlement 2026-12-30.
- Sell on **Saturday 2026-08-15** → `204`.

None of these days exists on the exchange's own calendar, which the database already holds and
which the settlement calculation reads on the very next line. The same calendar is *already*
enforced one entity away: `PUT /closing_prices/:listing/:date` refuses exactly this with
`422 2026-06-06 is not a trading day` (`closing_price::validate_complete_trading_day` over
`Market::is_trading_day`), and that helper resolves the calendar **as at the date** through the
rename chain and returns `true` unconditionally for an exchange-less (Crypto) listing, so it is
already the right shape for a trade too.

What rides on the trade date makes this more than a tidiness point: it is the CGT event date, so it
sets the 12-month discount clock, the financial year the gain falls in, and the day the T+n count
starts from. A date the market was shut is a data-entry error by construction.

**Live database: zero rows disagree** — no trade in Evan's 113 is dated on a weekend or on a seeded
holiday for its exchange, so a write-time refusal would leave every existing row editable.

- [x] Decide the shape (see the two options below) and implement it — (c) both. The refusal lives in
      `trade::db_upsert` and `sell::db_upsert_sell`, on the transaction each already opens (the
      calendar is a DB read, so it cannot go in the pure `check_amounts`), over the *existing*
      `closing_price` machinery: a new `non_trading_day(&Market, date)` beside
      `validate_complete_trading_day`, resolving the calendar as at the date through the rename
      chain and exempting exchange-less (Crypto) listings. `load_market` gained a
      `load_market_on(conn, …)` twin so the write path can read it inside its own transaction
      (`http::crud_get`, `listing::db_get`, `exchange::db_get` and `db_holiday_dates_for` are
      executor-generic for it). Deliberately **not** in `check_amounts` and **not** in
      `upsert_sell_in_tx`: a corporate action's own date may legitimately fall on a closed day
- [x] The derived Buy paths must be settled either way: `ess_vest` and `inheritance` **INSERT their
      Buy directly** rather than through `trade::db_upsert`, each carrying a module-doc list of
      "which `check_amounts` rejection is satisfied where" and an explicit instruction that *a new
      check added to `trade::check_amounts` needs a line here*. Both are dated by facts that are
      routinely **not** trading days — an ESS taxing point, and a date of death — so a check added to
      `check_amounts` must exempt them (and say why in those lists), or live outside it
      — done the second way: the check lives outside `check_amounts` entirely, and both module-doc
      lists carry a paragraph saying so and why (a taxing point is set by the scheme, a death keeps
      no exchange's hours), pointing at the health alert that covers them instead
- [x] `docs/API.md` — the 422 catalogue row and the trades/sells sections, or the health section
      — all of them: the catalogue row beside the pre-CGT/future-date bounds, a Trades paragraph
      (with the derived-path exemption), a Sells paragraph, and the `non_trading_day_trades` health
      entry; plus `docs/SCHEMA.md`'s `trades.date` comment and the README health bullet
- [x] Regression tests: weekend and seeded-holiday dates on `PUT /trades/:id` and `PUT /sells/:id`,
      the same Saturday accepted for a Crypto listing (the `L-15` shape), and a date in a year with
      no seeded calendar still accepted
- [x] The non-blocking half: `reports::health`'s `non_trading_day_trades`, over *every* trade rather
      than only the ones the refusal sees, naming the reason (weekend / holiday), the exchange whose
      calendar was in force on the date, and the write path that created the row

**Decision (Evan, 2026-08-22): (c) both.** Refuse on `PUT /trades/:id` and `PUT /sells/:id`
(Crypto and the derived ESS-vest / inheritance paths exempt), **and** carry a non-blocking
`reports::health` alert so a non-trading-day row a derived path writes is still surfaced.
Rejected: (a) refusal alone (nothing would surface the derived paths' rows), (b) the alert alone
(weaker than the rule `closing_prices` already enforces on the same calendar). The accepted cost is
that an off-market allotment dated on a closed day can no longer be entered through `/trades`.

---

---

## SCENARIOS S-05: a stored settlement date is never checked against the trading calendar, and the live database has one that falls on a Saturday

The only rule an explicitly supplied `settlement_date` has to satisfy is that it is not before the
trade date (`AmountsError::SettlementBeforeTrade`). It is never checked against the exchange's
calendar, so a hand-entered settlement can land on a day the exchange is closed — and one has:

```
trade 9071  LAC (XNYS)  date 2021-03-25  settlement_date 2021-05-29
```

**2021-05-29 is a Saturday**, and the two dates are two months apart, so this is a hand-entered
value rather than anything `auto_settlement_date` produced. It is in
`share-tracker-2026-08-16-000000.db` today and no surface anywhere mentions it:
`GET /reports/settlement_holiday_coverage` only asks whether the window is inside the *seeded
coverage span*, never whether the settlement day itself is a trading day, and `reports::health` has
no settlement check at all.

The auto path can produce one too — see S-04 below, where a settlement computed with no seeded
calendar landed on **2028-04-17, Easter Monday**.

A settlement date that is not a trading day on the listing's own calendar is wrong by construction,
whoever wrote it, and the check needs no bookkeeping about *when* it was computed —
`closing_price::Market::is_trading_day` already answers it, as at the date, with Crypto exempt.

- [x] Flag every stored `settlement_date` that is not a trading day on the listing's calendar.
      `GET /reports/settlement_holiday_coverage` is the natural home — a third `coverage_status`
      (or a sibling field) beside `outside_holiday_coverage` / `no_holiday_coverage` — so the one
      report that exists to answer "is this settlement date trustworthy" answers the whole question
      — done as **both**, because the two questions are independent and one trade can answer both
      badly: a sibling `settlement_non_trading_reason` (`weekend` / `holiday` / null, over
      `closing_price::non_trading_day`, one `Market` load per listing on the report's own read
      transaction) carries the new answer, and `coverage_status` gained a third value
      `inside_holiday_coverage` for the rows now listed for the settlement question alone. The row
      filter changed with it: a trade inside coverage is emitted when its settlement is not a
      trading day, so the report no longer omits every in-coverage trade
**Decision (Evan, 2026-08-22): flag it, do not refuse a supplied value.** An explicit
`settlement_date` is a deliberate override the user is asserting, so trade 9071 stays editable and
untouched; only the *auto-computed* path is guaranteed to land on a trading day. Rejected: refusing a
supplied non-trading-day settlement (it would brick trade 9071 until it was corrected), and flagging
with no guarantee on the auto path.

Note what that leaves to build: with S-08 refusing a non-trading-day **trade** date, the auto path
already lands on a trading day wherever the calendar is complete — `add_business_days` skips seeded
holidays by construction. The only way it produces a closed day is a *missing* calendar, which is
S-04, and a trade cannot be refused for the calendar being incomplete. So this section's work is the
**flag**, and the auto-path guarantee is delivered by S-04's recompute job rather than by a new
refusal. Add a test pinning that the auto path cannot produce a non-trading day under a complete
calendar, so the guarantee is asserted rather than assumed.

- [x] A supplied `settlement_date` is **not** refused — the coverage-report status is what surfaces
      it (trade 9071 stays editable; correct it separately if it turns out to be a typo, checking the
      deployed database at `bigbrain.lan:3000` as well, since the copy in the repo is the 2026-08-16
      backup rather than the live file)
- [x] `docs/API.md` — the settlement-holiday-coverage section's contract sentence — rewritten as
      the two questions the report answers, saying what an empty report does *and does not* mean
      (it is not a claim that each stored date is what today's calendar would compute — that is
      S-04's, still open); plus the Trades section (a supplied value is stored as given), the
      `non_trading_day_trades` health entry's cross-reference, `docs/SCHEMA.md`'s
      `trades.settlement_date` comment, the README feature line, and the report's `desc` /
      third-status badge in the web UI. Pinned by
      `doc_checks::settlement_coverage_documents_both_questions_it_answers`
- [x] Regression tests: a supplied weekend settlement flagged, a supplied holiday settlement flagged,
      a Crypto same-day settlement on a Saturday **not** flagged — plus a trade that is *both*
      outside coverage and settling on a weekend, reported so both facts stay legible, and the
      auto-path guarantee this section asks for:
      `trade::tests::auto_settlement_never_lands_on_a_non_trading_day_under_a_complete_calendar`
      walks every trading day of both seeded calendars (2019–2027, ~4,500 settlements) and asserts
      each computed settlement is itself a trading day, skipping the windows that run past the end
      of coverage (the incomplete-calendar case, which is S-04's). The weekend test reproduces
      trade 9071 exactly; run against a copy of the 2026-08-16 backup, the report returns that one
      row and nothing else

---

---

## SCENARIOS S-04: seeding the calendar the coverage report asks for silences the report without correcting the settlement dates it flagged

`GET /reports/settlement_holiday_coverage` documents its own contract in `docs/API.md`:

> Trades fully inside coverage are omitted — an empty report means every settlement window was
> computed against a complete calendar.

That sentence stops being true the moment the user does the thing the report exists to prompt.
Driven against the running system:

1. `exchange_holidays` is seeded 2019–2027. A Buy dated **2028-04-13** (the Thursday before the 2028
   Good Friday) auto-computes to **2028-04-17**, skipping weekends only, and is correctly listed as
   `outside_holiday_coverage`.
2. The user seeds the 2028 XASX calendar — Good Friday 2028-04-14, Easter Monday 2028-04-17, and the
   rest.
3. The report now returns **nothing** for that trade. The coverage span covers 2028, so the window
   is inside it.
4. The stored settlement date is **still 2028-04-17**, which is now a seeded **Easter Monday**. The
   correct answer on the completed calendar is 2028-04-19.

So the report's guarantee inverts: it is honest only while the calendar is *incomplete*, and goes
quiet exactly when the missing calendar is supplied. Nothing recomputes the affected trades, nothing
records that a stored settlement was computed against a calendar that has since changed, and the
same hole is already documented one door down for a holiday **deletion** ("a trade re-saved
afterwards without an explicit `settlement_date` silently recomputes against the changed calendar")
— but that note is about a re-save *changing* a date, not about a stale date staying put.

The S-05 trading-day check above catches this particular instance (2028-04-17 is a seeded holiday),
but not the general one: a settlement computed one day early because the window contained a holiday
that is not the settlement day itself lands on a perfectly good trading day and stays wrong.

- [x] Decide the shape (see the options below) and implement it — the `settlement-recompute` job
      (`POST /jobs/settlement-recompute`, registered in `infra::scheduler::registry`, deliberately
      absent from `schedule.cron`, the `price-rebase` shape). It re-derives each settlement date
      through `auto_settlement_date` itself — the write path's own function, over the listing's
      **live** `exchange_mic` — so the job's answer is exactly where a re-save would put the date
      and the two can never disagree; that inherits the documented live-exchange limitation
      deliberately rather than quietly resolving the calendar differently. One transaction,
      idempotent, and the UPDATEs go through the ordinary audited-table triggers, so each
      superseded date stays in `row_history`.

      **What the finding's write-up does not say, and is the whole of the work:** "rewrite the
      auto-computed ones and leave the stated ones alone" was *not answerable from the schema*.
      `trades.settlement_date` is one plain column written by both paths and nothing recorded
      which, and no heuristic recovers it (a supplied date that happens to equal T+2 is
      indistinguishable from a computed one). So migration **0041** adds
      `trades.settlement_date_source` — `computed` / `stated` / `unrecorded`, CHECK-constrained,
      the project's provenance-column idiom (`price_as_observed`, `domain::rollover::Provenance`).
      Three values because there are three states: every **existing** row takes `unrecorded` from
      the ADD COLUMN default (no UPDATE, so the migration writes no audit rows and stales no
      snapshots), and the job never rewrites `stated` or `unrecorded` — guessing could overwrite an
      assertion like trade 9071's. The default is the never-rewritten value, so a write path that
      forgets the column can only under-claim. The derived paths (ESS vest, inherited parcel, DRP
      reinvestment, rights exercise, every rollover trade) name `'stated'` in their INSERT: their
      same-day settlement is asserted by construction. One qualification keeps the provenance
      meaningful: **re-supplying the date already stored keeps the recorded source**, because a GET
      body PUT back verbatim is what the web UI's edit form sends, and treating that as an
      assertion would opt every edited trade out of the repair (pinned by
      `entities::tests::what_a_get_returns_can_be_put_back_unchanged`, which caught it).

      Cost on the live database: nil and provable. All 113 rows become `unrecorded`, and none of
      them needs recomputing anyway — every settlement window is inside the seeded 2019–2027
      coverage. Run against a copy of `share-tracker-2026-08-16-000000.db`: migration 0041 applies,
      the job answers `204` logging `trades=113 candidates=0 recomputed=0`, and every settlement
      date, every `row_history` row and every snapshot's `stale` flag is byte-identical to the
      original — trade 9071 included
- [x] `docs/API.md` — the coverage-report contract sentence is currently false and must be corrected
      whichever option is taken — it now names the repair ("**Run the `settlement-recompute` job**
      after seeding a calendar"), the Jobs list documents the job as unscheduled and says what it
      will not rewrite, the Trades section documents the read-only `settlement_date_source` field
      and the re-supply rule, and the live-exchange Known-limitation says the job inherits it.
      Plus `docs/SCHEMA.md`'s new column line, the README feature line and its unscheduled-job
      paragraph, the Jobs-screen description and the Trades screen (the new column, its
      `COLUMN_LABELS` heading, and the settlement-date field hint). Pinned by
      `doc_checks::settlement_recompute_job_documented`
- [x] Regression tests: the four-step reproduction above, and a trade whose stored settlement still
      matches a recomputation staying silent — five, in `entities::trade::tests`:
      `seeding_a_missing_calendar_and_recomputing_corrects_the_settlement_it_left_wrong` is the
      four-step reproduction end to end through the API (transposed to the 2018 Easter, because
      S-10 now refuses a trade dated 2028 — same shape, missing calendar at the other end of the
      seeded span: settles on the unseeded Easter Monday, the year is seeded, the stored date does
      not move, the job re-derives it to 2018-04-04, the report empties, and the superseded date is
      in `row_history`); `recompute_corrects_a_settlement_left_a_day_early_by_a_missing_holiday` is
      the *general* case S-05 cannot catch (only the in-window holiday missing, so the stored date
      is a perfectly good trading day and the report is silent);
      `recompute_leaves_a_settlement_that_already_matches_the_calendar_untouched` (no write at all,
      so no audit row — which is also what makes the job idempotent);
      `recompute_leaves_a_hand_supplied_settlement_untouched` reproduces trade 9071's shape (LAC on
      XNYS, 2021-03-25 → 2021-05-29) and asserts it survives the job, still flagged, unaudited; and
      `recompute_leaves_a_row_from_before_the_provenance_column_untouched` pins the `unrecorded`
      default, including that a verbatim re-save keeps it and entering a different date does not

**Decision (Evan, 2026-08-22): (b) a `settlement-recompute` job** — registered in
`infra::scheduler::registry` and deliberately **unscheduled** (the `price-rebase` shape from Q-14),
rewriting auto-computed settlement dates from the current calendar, with the docs saying to run it
after seeding a calendar. Rejected: (a) recompute-and-compare inside the report (it would have to
distinguish a deliberate override such as trade 9071 from a stale computation, and it reports rather
than repairs), and (c) documentation only. The report's contract sentence still has to be corrected
either way, and the job needs to leave a hand-supplied `settlement_date` alone — see S-05, where the
supplied value is the user's own assertion.
