# Done — Web Frontend

## Web Frontend
A no-build-step single-page app (plain HTML/CSS/JS) embedded in the binary with `include_str!` and served by axum (`src/web.rs` + `src/web/{index.html,app.js,style.css}`, merged in `app::router`). The SPA is config-driven: each domain entity is described once (API path, key, fields) and generic list/form code renders its CRUD view; reports render as tables. It drives the existing JSON API on the same origin, so there is no second source of truth. Tests live in `src/web::tests`: the served shell/assets return the right status + content-type, and — since there is no browser harness — each UI item is covered by asserting its view (and the API endpoint it drives) is present in the shipped `app.js` bundle.
- [x] Serve frontend from the Rust server (axum) — `web::router` serves `GET /`, `/static/app.js`, `/static/style.css` with correct content-types (`index_is_served_as_html`, `app_js_is_served_as_javascript`, `style_css_is_served_as_css`)
- [x] Exchange management UI — generic CRUD view over `/exchanges` (`exchange_management_ui_present`)
- [x] Listing management UI — generic CRUD view over `/listings`, with exchange/currency dropdowns (`listing_management_ui_present`)
- [x] Trade entry and listing UI — generic CRUD view over `/trades` for Buy/DRP (Sells excluded — entered via the Sells view); optional settlement date auto-calculates (`trade_ui_present`)
- [x] Income entry and listing UI — generic CRUD view over `/income`, full tax-component fields (`income_ui_present`)
- [x] AMMA statement entry and listing UI — generic CRUD view over `/amma_statements` (`amma_statement_ui_present`)
- [x] Share parcel allocation UI — bespoke Sells view: a Sell trade form with a dynamic allocations list, submitted atomically via `PUT /sells/:id`; `parcel_allocations` shown read-only (`parcel_allocation_ui_present`)
- [x] DRP enrolment management + reinvest-distribution UI — CRUD over `/drp_enrolments` (keyed by listing); income rows expose a Reinvest action driving `POST /income/:id/reinvest` (`drp_enrolment_ui_present`, `income_ui_present`)
- [x] Portfolio overview UI — `/portfolio/overview` report view with a per-listing price form (`portfolio_overview_ui_present`)
- [x] Gains/losses report UI — `/portfolio/unrealised-gains` (price + as-of-date form), `/portfolio/realised-gains`, and `/portfolio/net-capital-gain` report views (`gains_report_ui_present`)
- [x] Tax summary UI — `/portfolio/tax-summary` report view (`tax_summary_ui_present`)
- [x] Attachments UI on the Trade / Income / AMMA views — each of those entities carries an `attachOwner` field name; the generic list adds an "Attachments" row action linking to `#/attachments/<owner>/<id>`. `viewAttachments` lists an activity's attachments through the shared `filterableTable`, uploads a new file via `FormData` → `POST /attachments` (browser sets the multipart boundary + part content-type), links each row to its download (`/attachments/:id/content`), and deletes. Test `web::tests::attachments_ui_present` asserts `viewAttachments` + `/attachments` + `attachOwner` ship in the bundle (no browser harness)
- Also wired into the SPA (no separate TODO item): read-only views for currencies / MIC registry / RBA FX rates / parcel allocations, the AMIT adjustments CRUD view, exchange holidays CRUD, the exchange MIC validation report, and a Maintenance → Jobs view that lists and triggers the scheduled jobs on demand via `GET /jobs` + `POST /jobs/:name` (`jobs_ui_present`).
- [x] Web UI tables are filterable and sortable (REQUIREMENTS: "Tables in the Web UI should be filterable and sortable") — shared `filterableTable(rows, cols, opts)` in `app.js` renders every data table (entity lists, the Sells list, and report tables via `dataTable`) with a per-column filter row (one input per column, substring match, AND-combined so e.g. currency="USD" + date="2024" filter together) and click-to-sort headers (toggle asc/desc, numeric columns sort numerically). `opts.actions` keeps the trailing Edit/Delete column non-sortable/non-filtered; no per-entity code, no parallel data path
- [x] Tests: the filter input and sortable-column controls are present in the served `app.js` bundle (`web::tests::tables_are_filterable_and_sortable` asserts `filterableTable`/`table-filter`/`sortable` ship in the bundle, per the no-browser-harness convention)

## Web Frontend — config-driven refactors
(From a 2026-06-07 review of `src/web/app.js` against the question "is it time to migrate to a web framework?" — answer: no, the config-driven no-build-step approach is holding up well (bespoke views ~7 vs config-driven ~18), but three duplication strains are worth fixing *within* the current approach so the bespoke count stops growing. Pure frontend refactors: no API or schema changes, behaviour identical; existing `web::tests` bundle assertions must keep passing.)
- [x] Extract a shared allocation-editor helper: the purchase-parcel + quantity allocation-row builder (`addAllocRow`: select over `buyParcels` options + decimal quantity input + Remove button) and the matching `querySelectorAll('.alloc-row')` submit-time harvest are copy-pasted three times — `viewSellForm`, `viewTransferForm`, and `viewParticipate`. Replace with one `allocationEditor(parcelOptions, existingAllocs, labels)` helper returning the section element and a `read()` → `[{purchase_trade_id, quantity_allocated}]` function; the three views differ only in labels ("Purchase parcel"/"Parcel to move") and hint text. Tests: `web::tests` asserts the helper ships in the bundle and each of the three views drives it (per the no-browser-harness convention); the existing `parcel_allocation_ui_present` / transfers / participate bundle assertions still pass — done: `allocationEditor` in `app.js` (labels default to the Sell wording; Transfer overrides all of them, Participate only the hint); `web::tests::parcel_allocation_ui_present` asserts the definition plus exactly three call sites
- [x] Make the post-action forms config-driven like entities are: `viewReinvest`, `viewExercise`, `viewParticipate`, `viewScripExchange`, and `viewDemerge` all repeat the same template — fetch the owning record, render fields, POST to an action endpoint, toast the created id(s), redirect — differing only in fields, endpoint, description, and toast text. Describe each in an `ACTIONS` config (slug, owner fetch, fields, allocations? flag reusing the shared allocation editor, POST path, desc/toast builders) driven by one generic `viewAction` renderer, mirroring how `ENTITIES` drives `viewEntityForm`; the no-field confirm-only actions (scrip exchange, demerge) are the degenerate config with `fields: []`. Tests: `web::tests` asserts the `ACTIONS` config and generic renderer ship in the bundle with each action's POST endpoint present; existing per-action bundle assertions still pass — done: five `ACTIONS` entries + `viewAction` in `app.js`; the router dispatches `#/<slug>/<id>` via `actionBySlug`. Tests: `web::tests::post_actions_are_config_driven` (config + renderer + each slug and POST endpoint); the four `corporate_actions_ui_present` assertions on the removed `viewExercise`/`viewParticipate`/`viewScripExchange`/`viewDemerge` function names now assert the equivalent row-action hrefs (`#/exercise/` etc.) — every endpoint/field assertion unchanged
- [x] Split the corporate-actions form by action type: the single `corporate_actions` form renders all 20 type-specific fields at once (each hinted "X only", mostly blank for any given action) and the desc paragraph explains all seven types in one block. Show only the chosen `action_type`'s fields — re-render the type-specific fields on action-type change within the existing `buildFieldInput` machinery (e.g. a per-type field-group map on the entity config), and scope the description text to the selected type. Edit mode pre-selects the row's type. The server contract is unchanged (unchosen types' fields submit as null, as the blank inputs do today). Tests: `web::tests` asserts the per-type field grouping ships in the bundle (and `corporate_actions` + its field names remain present) — done: `typeField`/`fieldGroups`/`typeDescs` on the `corporate_actions` entity config, rendered generically by `viewEntityForm` (common fields = those in no group; unrendered fields submit as null; the now-redundant "X only" hints dropped). Tests: `web::tests::corporate_action_form_is_split_by_type` plus the existing `corporate_actions_ui_present` field-name assertions
- [x] Fix the findings from the 2026-06-07 browser verification of the type-split corporate-actions form: (1) the async type-group render (fk fields fetch their options) had no stale-render guard — a slow render could append its fields after a newer selection's; a `renderSeq` sequence number makes the stale render abandon itself, and the group now builds off-DOM and swaps in atomically. (2) Flipping the action type away and back discarded what the user had typed (re-read from the stored row) — outgoing inputs are harvested into a `draft` that wins over the stored row on re-render. (3) Editing a row to a different type silently nulled the old type's fields on save — an amber `.hint.warn` line ("Saving as X clears the saved Y fields.") now appears whenever the selected type differs from the saved one. (4) The date label was still the seven-way "Payment / conversion / issue / … date" — a `typeLabels` config map scopes it per type ("Payment date", "Conversion date", …; generic "Date" until a type is chosen). Tests: `corporate_action_form_is_split_by_type` extended to assert `renderSeq`, `const draft`, the warning text, and `typeLabels` + per-type date labels ship in the bundle; behaviour re-verified in headless Chrome incl. a 700ms-throttled `/listings` race probe

## Web UI readability adjustments
(User feedback 2026-06-07 from entering a trade through the web UI: foreign-key id columns in the list tables read as bare integers, and the attachment checksum is implementation metadata nobody needs on screen. UI-only — no API or schema changes.)
- [x] List tables show the referenced row's name for foreign-key id columns instead of the raw id — listings as `MIC:TICKER` (e.g. `XNYS:ICE`; Crypto listings have no MIC and render `Crypto:BTC`), holding accounts by name (e.g. `Default`) — across the generic entity lists (trades, income, AMMA, DRP enrolments, corporate actions, …) and the bespoke Sells and Transfers lists; column filtering and sorting follow the displayed name, and the raw id stays reachable on the cell tooltip (`title="id N"`). Currency codes and trade/statement ids are left as-is (already readable / no natural name) — `TABLE_LABEL_SOURCES` + `fkLabelMaps` in `app.js` feeding a new `labels` option on the shared `filterableTable`; `viewEntityList` derives the maps from each entity's existing fk field config (no per-entity wiring), Sells/Transfers pass theirs explicitly. Verified rendered in headless Chrome 2026-06-07 (trades row shows `XNYS:ICE` / `Default` with `id 1` tooltips). Test: `web::tests::fk_columns_render_names_not_ids`
- [x] Attachments list no longer shows the `checksum` column — it stays stored (SHA-256 integrity metadata, returned by the API) but is not user-facing. Test: `web::tests::attachments_ui_present` asserts no `'checksum'` column ships in the bundle
- [x] Corporate-action follow-up pages (Reinvest / Exercise / Participate / Exchange / Demerge) name the listings in their descriptions instead of printing raw ids — e.g. the scrip-exchange page now reads "Substitutes every open parcel of XNYS:ICE … with 1 unit(s) of XNYS:NEWCO" — `viewAction` builds a `listingName` resolver from the shared `fkLabelMaps` and passes it as each `desc(owner, listing)`'s second argument (unknown/null ids fall back to the old "listing N" wording). The corporate-actions *list* columns (`scrip_listing_id`, `demerger_listing_id`) were already covered by the previous item's generic fk derivation — re-verified rendered in headless Chrome 2026-06-07 (list row shows `XNYS:NEWCO` with `id 2` tooltip; action page shows the sentence above). Test: `web::tests::fk_columns_render_names_not_ids` extended (desc resolver wiring + the scrip/demerger resolver calls + no raw-id reinvest wording)
- [x] The allocation-editor parcel options (Sell / Transfer / Participate forms) and the AMMA-statement options (AMIT-adjustments form) name the listing instead of printing the raw id — "1: Buy 10 (XNYS:ICE, 2026-05-01)" rather than "(listing 1, …)", "1: XNYS:VAS FY2025-06-30" rather than "1: listing 2 FY…" — the id→`MIC:TICKER` resolver is extracted into a shared `listingNamer()` (unknown/null id falls back to "listing N") used by `loadOptions('buyParcels')`, `loadOptions('amma')`, and `viewAction`'s descriptions. Verified rendered in headless Chrome 2026-06-07 (Sell form parcel option shows the ticker wording). Test: `web::tests::fk_columns_render_names_not_ids` extended (resolver + both option call sites + the old raw-id label wordings absent from the bundle)

## No raw foreign keys shown in the web UI
(REQUIREMENTS "New Requirements — No raw foreign keys shown in the web UI", added 2026-06-07. Extends the 2026-06-07 "Web UI readability adjustments" section — list tables, the post-record action descriptions, and the allocation-editor/AMMA option labels already render names via the shared `fkLabelMaps`/`listingNamer` — into an audited invariant over *every* surface. UI-only: no API or schema changes.)
- [x] Audit every web surface for a raw id standing in for an entity: entity list tables, report tables, form `<select>` option labels, read-only/derived fields on forms, the post-record action pages and their option labels, confirmation prompts, and toast messages (e.g. "Reinvested into trade #12", "via transfer-out sell #7"). Record each finding here (fixed vs already-named) so the audit is repeatable. **Findings (2026-06-08):**
  - *Entity-list tables* — previously only the `fk` *fields* with a `TABLE_LABEL_SOURCES` source were named, so columns without an editable field showed raw ids: income's `reinvestment_trade_id`, parcel-allocations' `sale_trade_id`/`purchase_trade_id`, and amit-adjustments' `amma_statement_id`/`trade_id` (their sources `amma`/`buyParcels` weren't in the label map at all). **Fixed** — naming is now driven by *column name* (`FK_COLUMN_SOURCES` → `columnLabelMaps`), covering every id column whether or not it has a field.
  - *Report tables* — `dataTable` rendered every report column as raw text, so `listing_id`/`holding_account_id`/`trade_id`/`sale_trade_id`/`account_id` across open-parcels, realised-gains, portfolio, unrealised, performance, settlement-coverage showed raw ids. **Fixed** — `dataTable` now applies `columnLabelMaps(cols)` (the same path as the lists). Verified in headless Chrome (open-parcels shows `Buy 100 XASX:VDHG on 2024-01-10` / `XASX:VDHG` / `Default`, raw id on the tooltip).
  - *Toasts* — reinvest ("Reinvested into trade #N"), exercise, participate ("…as trade #N with dividend income #N"), scrip-exchange/demerge ("via closing sell #N"), the income form's chained reinvest, and the transfer ("via transfer-out sell #N") reported only ids. **Fixed** — see the toast item below.
  - *Action page titles* — "Reinvest distribution #5", "Demerge #5", etc. named no entity. **Fixed** — titles now lead with the listing (`title(id, owner, listingName)`).
  - *Attachments view header* — "Files attached to trade #5". **Fixed** — names the owning activity (trade description / distribution / AMMA + listing & date), id as secondary.
  - *Closing-prices list* — the `listing` column led with the raw id (`5: ICE`). **Fixed** — shows `MIC:TICKER`.
  - *Already-named (no change needed):* all `<select>` option labels (`loadOptions`) already show the name with the id alongside (listings `id: TICKER (MIC)`, buyParcels full trade description, amma, holdingAccounts, currencies, exchanges); the post-record action *descriptions* and the allocation-editor/AMMA option labels (done in the earlier readability section); report price-form `<label>`s (`id: TICKER (MIC)`); confirmation prompts (no ids); `currency`/`exchange_mic` columns (natural readable codes, not numeric ids).
- [x] Apply the naming convention to each finding: listing → `MIC:TICKER` (Crypto listings `Crypto:TICKER`), holding account → name, trade/parcel → a human description (side, quantity, ticker, date — an id alone is meaningless), other entities → their most recognisable label. An id may appear *alongside* the name (e.g. the existing `title="id N"` tooltip) but never instead of it — implemented via `FK_COLUMN_SOURCES` (column-name → source) + `TABLE_LABEL_SOURCES` (now incl. `trades` via the shared `describeTrade(t, listingName)` = "side qty MIC:TICKER on date", and `amma` = "MIC:TICKER FY…", both resolved in dependency order so a trade names its listing) + `columnLabelMaps`, shared by the entity lists, Sells/Transfers lists, and report tables; the raw id stays on the cell `title` tooltip
- [x] Toasts that today report only a created row's id name what was created (e.g. the reinvestment Buy's ticker, quantity, and date), with the id as secondary detail — reinvest/exercise/participate now toast `describeTrade(...)` + `(trade #N)`; participate's dividend income names its listing; scrip-exchange/demerge name the head + target listing (closing sell id secondary); the transfer toast names the listing + both accounts; the income form's chained reinvest names the created DRP trade. `viewAction` passes the `listingName` resolver + owner to the `toast`/`title` config fns
- [x] Tests: each fixed surface's rendering path asserted in the served bundle (no-browser-harness convention), extending `web::tests::fk_columns_render_names_not_ids` or siblings; spot-verify rendered output in headless Chrome — `fk_columns_render_names_not_ids` rewritten (column-name resolver, `describeTrade`, trade/amma id columns, report tables via `columnLabelMaps`, named action titles) + new `toasts_and_attachments_name_what_was_created_not_just_an_id`; spot-verified in headless Chrome (income list reinvestment-trade column + open-parcels report render names with the id on the tooltip)
- [x] Docs sync: README web-frontend paragraph if the user-visible behaviour changes meaningfully — added a "Names, never raw ids" paragraph to `docs/API.md` Web frontend (the convention, the per-entity name forms, id-as-secondary, display-only)

## Currency rounding in lists and reports; precision where it matters
(REQUIREMENTS "New Requirements — Currency rounding in lists and reports; precision where it matters", added 2026-06-07. Display-only rounding: the JSON API keeps returning full-precision Decimal strings — rounding lives in the web layer's formatters, so API consumers, CSV exports, and the server-side cross-checks are unaffected. Aggregation always happens at full precision; rounding is applied only at the formatting step. Amounts round to the minor unit; per-unit *rates* keep their precision because rounding them breaks statement reconciliation.)
- [x] Classify every displayed numeric column across the entity lists and report tables as monetary amount / per-unit rate / quantity — a `COLUMN_KINDS` map in `app.js` keyed by **column name** (not per-config, so it is shared across the JSON API: a column reused on a new screen classifies once) maps each numeric column to `money` / `rate` / `quantity` (+ `rate4` for derived per-unit figures); `columnKinds(cols)` is the synchronous analogue of `columnLabelMaps`, derived inside `filterableTable` from its `cols` so every caller — the generic entity lists, the bespoke Sells/Transfers lists, and the report `dataTable` path — inherits the rule with no per-call wiring. Numeric columns absent from the map (ids, financial years, counts like `settlement_days`/`byte_size`, percentages like `*_pct`/`demerger_cost_base_pct`) display verbatim — never wrongly rounded. Every report struct field and entity column was enumerated and classified
- [x] Format monetary amounts to 2 decimal places (half away from zero) in the shared formatter, with thousands grouping; exact decimal-string arithmetic (BigInt), never `parseFloat` on money — `roundDecimalStr(value, dp)` (signed, half-away-from-zero via BigInt remainder doubling) + `groupThousands` (regex on the integer part) in `numericDisplay(value, 'money')` (e.g. `19.995` → `20.00`, `123476.775` → `123,476.78`, `-1234.565` → `-1,234.57`, `-0.001` → `0.00` not `-0.00`). The JSON API and CSV exports keep full precision — rounding lives only at the cell render
- [x] Per-unit rates and quantities keep their entered/natural precision; derived per-unit figures display with at least 4 decimal places, not cent-rounded — `rate`/`quantity` kinds render the value verbatim (`average_price`, `amount_per_security`, `reinvestment_price`, `fx_rate`, `rate`, `price`/`current_price`, the buy-back/ROC/exercise per-unit fields, `cost_base_adjustment`; all the `*units`/`quantity*`/`securities_held` columns); the one derived per-unit report figure `avg_cost_base_per_unit` is kind `rate4` → `padMinDp(s, 4)` shows ≥4 dp (`5` → `5.0000`, `5.123456` kept). Verified in headless Chrome: a trade's `1234.5678` average price renders verbatim, a `100` quantity verbatim
- [x] Column sorting and filtering still operate on the underlying numeric value where they did before — numeric sort still compares the raw `row[sortCol]` via `Number()` (a money column is still `numeric` since its raw value `looksNumeric`), so sorting is by full precision; filtering matches the displayed (formatted) text — documented in the new docs paragraph ("filters match the displayed text"). Formatting flows through the single `displayText`, so filter + string-sort + cell render stay consistent
- [x] Tests: the formatter + per-column kinds ship in the bundle and the shared table path applies them; spot-verify a rounded amount and an unrounded rate render correctly in headless Chrome — `web::tests::currency_amounts_round_in_tables_rates_keep_precision` asserts `COLUMN_KINDS`/`columnKinds`/`roundDecimalStr`/`groupThousands`/`padMinDp`, the representative money/rate/rate4/quantity classifications, and the `numericDisplay(row[c], kinds[c])` + `nd.tip` wiring ship in the bundle. Headless-Chrome (`--dump-dom`, no puppeteer needed) spot-verified against a live seeded server: the trades list `brokerage` cell shows `20.00` with `title="19.995"` while `average_price` renders `1234.5678` verbatim, and the open-parcels report `original_cost_base` shows `123,476.78` with the full `123476.7750` on its tooltip. (Per the user's request mid-task: a money cell rounded for display carries the full original value on its hover tooltip — `numericDisplay` returns `tip` when `decStrEq(rounded, original)` is false.)
- [x] Docs sync: the amounts-round-rates-don't rule documented once — an "Amounts round, rates don't" paragraph added to the `docs/API.md` Web frontend section (the per-column-kind classification, 2 dp + grouping for money, full precision for rates/quantities, ≥4 dp for derived per-unit figures, display-only/API-unchanged, sort-on-raw + filter-on-displayed, and the rounded-money hover tooltip)


## Useful error messages in the web UI
(REQUIREMENTS "New Requirements — Useful error messages in the web UI", added 2026-06-07. A rejected write must say *why*: today most handlers return a bare `StatusCode`, so the toast reads just "HTTP 422" — e.g. reinvesting a distribution for an account not DRP-enrolled (`drp_reinvestment` `NotEnrolled`) gives no hint that enrolment is the problem. The toast plumbing already displays a response body when present (`api()` in `app.js` appends it), so the work is server-side; the per-share (`per_share_detail`) and statement-total (`statement_total_detail`) 422 bodies are the model.)
- [x] Audit every handler that returns a bare `StatusCode` for user-triggerable 422/409/404-with-a-cause responses; inventory each endpoint's rejection causes (the entity upserts' `write_error_status` constraint mapping, the Sell/transfer/operation invariants, DRP reinvestment, corporate-action freezes, delete-while-referenced refusals, …) — done across every entity/operation handler: corporate_action, sell, transfer, demerger, drp_reinvestment, drp_enrolment, rights_exercise, scrip_exchange, buyback_participation, amit_adjustment, listing, holding_account, exchange, exchange_holiday, cgt_settings, amma, income, attachment, trade, currencies/mic_registry/rba_fx_rate imports
- [x] Attach a short, human-readable, plain-text body to each: which invariant failed, with the actual values involved (e.g. "allocations sum to 95 but the sell quantity is 100", "account 'Broker' is not enrolled in a DRP for VDHG at 2026-03-04 — enrol it on the DRP enrolments screen first"); messages name entities by name/ticker, never by raw foreign-key id (per the no-raw-ids section above) — every handler now returns `(StatusCode, String)`; the flagship `drp_reinvestment::NotEnrolled` was enriched to carry the account name, ticker, and entitlement date (looked up inside the tx) and formats exactly the example message; the shared `infra::http::write_error_body` maps a constraint violation to `422` + the DB's own (column/constraint-naming, never value-leaking) message and logs+generic-`500`s anything else (the old status-only `write_error_status` was removed — all call sites now surface a body)
- [x] 404s from a stale UI say what wasn't found (a bare "not found" body is fine where there is nothing more to say); 5xx responses stay generic — internal details go to the server log, not the toast — `NotFound`/`IncomeNotFound`/`ActionNotFound` arms carry "no <entity> with that id"; every `500`/`Db` arm returns an empty body and logs via `tracing::error!` (in the handler or `write_error_body`)
- [x] Tests: each validated rejection asserts the body text (or a distinctive fragment of it), not just the status code — extend the existing 422 tests as each handler gains its detail — body-fragment assertions added across representative handlers: drp_reinvestment not-enrolled (asserts "Default", "T1", "not enrolled"), sell under-allocation, transfer same-account, amit not-Buy/DRP, drp_enrolment overlap, cgt negative loss, holding_account duplicate-name + still-has-data, listing unrecognised-digital-token; the rights_exercise action helper asserts every client-error carries a non-empty reason body
- [x] Docs sync: `docs/API.md` Response codes section documents the plain-text error-body convention; each endpoint's non-obvious 422 causes listed where they aren't already — an **Error bodies** paragraph added under the Response codes table (plain-text body on every client-error/`502`, names entities not ids, constraint message surfaced, `5xx` stays generic); per-endpoint 422 causes were already enumerated

## Human-friendly headings and field labels throughout the web UI
(REQUIREMENTS "Human-friendly headings and field labels throughout the web UI", added 2026-06-08. Every heading, table column header, and form field label shown to the user must read as a human-friendly name, not the raw database/JSON field name — `amount_per_security` → "Amount per security", `exchange_mic` → "Exchange", `fx_rate` → "FX rate", `holding_account_id` → "Account". The labelling counterpart to the no-raw-foreign-keys requirement: that fixed raw id *values*; this fixes raw field *names* in the chrome around them. Config-driven in `app.js`, declared once per field; UI-only — no API/schema changes.)
- [x] A config-driven label mapping living with the existing per-entity/report config in `app.js` (`ENTITIES`/`REPORTS`/`ACTIONS` descriptors): labels declared once per field, read by the generic list/form/table code — not hand-written per view — table headers and filter placeholders now read through the shared `columnLabel(c)`, backed by a `COLUMN_LABELS` override map keyed by column name (sitting beside `COLUMN_KINDS`/`FK_COLUMN_SOURCES`, the established name-keyed pattern shared across the JSON API); form input labels and screen/section headings already read from their per-field `label` / entity `title` config, so no raw name leaks there
- [x] A default humaniser so a field with no explicit label never renders a raw identifier: snake_case → "Title case", with acronyms kept in canonical casing (AUD, FX, MIC, DRP, CGT, AMIT, GST, LIC, FITO) rather than "Aud"/"Drp" — `humanizeLabel(name)` drops a trailing `_id` (the cell already shows the referenced row's name, so `listing_id` → "Listing"), sentence-cases the words, and maps each word through `LABEL_ACRONYMS` for canonical casing
- [x] Apply friendly labels across all surfaces: `filterableTable` column headers, form input labels, report table headers, and section/screen headings — `filterableTable` (the one renderer behind every entity list, the Sells/Transfers lists, the jobs/closing-prices/snapshots tables, and the report tables) now titles each header and filter via `columnLabel`; verified end-to-end with `scripts/ui-check.sh` (the trades list renders "ID / Trade type / Settlement date / Listing / Average price / FX rate / Account")
- [x] Units/qualifiers shown in the label where they aid reading (e.g. "Price (AUD)", "Quantity (units)") without changing the underlying field name — the always-AUD report aggregates carry an "(AUD)" qualifier in `COLUMN_LABELS` ("Market value (AUD)", "Total cost base (AUD)", "Proceeds (AUD)", "Capital gain/loss (AUD)", "Unrealised gain/loss (AUD)", "Net capital gain (AUD)", "Average cost base per unit (AUD)"); per-row entity tables deliberately get no currency qualifier because their amounts are in the row's own currency column
- [x] Tests (served-bundle convention): assert the friendly labels render and that no raw field name leaks into a heading/label — `web::tests::column_headings_are_human_friendly` asserts `humanizeLabel`/`COLUMN_LABELS`/`columnLabel` ship, the header + placeholder read through `columnLabel(c)` (and the raw `[c, indicator]` / `'Filter ' + c +` forms are gone), the acronym set and `_id`-stripping are present, and the called-out overrides (`exchange_mic` → "Exchange", `holding_account_id` → "Account") plus the "(AUD)" qualifier render. Docs: `docs/API.md` gained a **Human-friendly headings** note beside *Names, never raw ids*

## Client-side pagination for large tables
(REQUIREMENTS "Client-side pagination for large tables", added 2026-06-08. Tables that can grow large — entity lists, the Sells list, report tables (trades, closing-price history, snapshots, parcels) — should paginate so a long result set isn't dumped as one table. Client-side at this stage: the JSON endpoints keep returning the full array and the web layer pages through it. Server-side API pagination is out of scope for now — record as a Known limitation.)
- [x] The shared `filterableTable` gains pagination: a 50-row default page size with page navigation (next/prev and/or page numbers), so only one page of rows is in the DOM at a time; tables of 50 rows or fewer show no pagination control — a module-level `PAGE_SIZE = 50` constant; `renderBody` slices `vr.slice(start, start + PAGE_SIZE)` for the current `page` into the DOM, and `updatePager` shows a prev/next pager only when the filtered total exceeds one page (prev disabled on the first page, next on the last)
- [x] Pagination composes with filtering and sorting: filtering/sorting apply to the **whole** result set, then the result is paged (never page-then-filter); changing a filter resets to the first page; the count reflects the filtered total and the control shows e.g. "showing 1–50 of 320" — `visibleRows()` builds the full filtered+sorted set first and only `renderBody` pages it; the filter `oninput` sets `page = 0` before re-rendering; the page is clamped into range after a filter shrinks the set; `pager-info` reads "showing m–n of total" off the filtered length
- [x] Applied uniformly through `filterableTable` so every table benefits without bespoke per-table paging — paging lives entirely inside the one shared renderer (entity lists, the Sells/Transfers lists, the jobs/closing-prices/snapshots tables, and the report tables), no per-call wiring
- [x] Docs sync: record server-side API pagination as a Known limitation (`docs/API.md` / README) — added a **Server-side pagination** bullet to `docs/API.md`'s Known limitations: the list/report endpoints always return the full array (no `limit`/`offset`/cursor), the UI pages client-side, so this addresses rendering/usability, not payload size
- [x] Tests (served-bundle convention): assert the paging controls/behaviour ship in the bundle and that filtering still reflects the full result set — `web::tests::tables_are_paginated` asserts `PAGE_SIZE = 50`, `updatePager`, the "showing"/prev/next text, the `vr.slice(start, start + PAGE_SIZE)` + `pageRows.forEach` paging, the filter `page = 0` reset, the one-page hide guard, and the `.pager`/`.pager-info` styles in the CSS bundle

## Proactive job-failure and data-staleness surfacing in the UI
A failing price import or RBA FX import is only visible if the Jobs page is opened; meanwhile
valuations silently go stale (yfinance is an unofficial API and will break eventually). `job_runs`
keeps only the last run per job (`scheduler::db_record_run` overwrites), so an intermittent failure
that later succeeds leaves no trace.
- [x] Health/freshness endpoint: latest closing-price date, latest RBA FX rate month, and any job whose last run errored, in one read (report-style, single read transaction) — `GET /reports/health` (`reports/health.rs`) reads the latest ok closing-price date, the latest RBA FX month, and every job whose most recent run failed on one `pool.begin()` snapshot; staleness is computed server-side so the UI stays dumb and the thresholds are pinned by tests — prices stale past 3 business days (a coarse Mon–Fri count, deliberately not the per-exchange holiday calendar: a freshness alarm, not a settlement calculation), FX stale when the latest month is older than the previous calendar month (RBA publishes month M shortly after M ends); a series with no data at all is not stale — a fresh install shows no banner, and an import broken before ever succeeding surfaces via `failed_jobs` instead
- [x] Web UI banner/strip on the main views driven by that endpoint: show stale price/FX data (threshold-based, e.g. prices older than N business days) and any failed job, linking to the Jobs page — `refreshHealthBanner` (app.js) populates the `#health-banner` strip (index.html, styled in style.css) on every route render, naming each problem and linking to `#/jobs`; a failing health fetch hides the banner rather than breaking the app; verified live: with a failed price-import run recorded the banner shows "⚠ Job 'price-import' failed: yahoo returned 403." on every view, and stays hidden on a healthy database
- [x] Bounded per-job run history (e.g. last 20 runs per job, pruned in the same write) so flapping jobs are diagnosable; `GET /jobs` exposes it; migration extends/replaces the single-row `job_runs` shape without dropping data — migration `0012_job_run_history.sql` rebuilds `job_runs` as append-per-run (autoincrement id + `(name, id)` index) via the rename pattern, carrying every existing row forward as its job's first history row; `db_record_run` inserts and prunes to the newest `JOB_RUN_HISTORY_LIMIT` (20) rows per job in one transaction; `GET /jobs` gains `runs` (most recent first) with the `last_*` fields mirroring `runs[0]`; the Jobs screen expands each job row to its run history through the shared `filterableTable` expand machinery
- [x] Tests: staleness thresholds (fresh vs stale), failed-job surfacing, history bound enforced, UI binding asserted in the served bundle — `reports::health::tests` (business-day / previous-month unit tests, fresh-vs-stale exactly at the 3-business-day boundary, errored price rows don't count as fresh, FX fresh/stale/current-month, failed-latest-run surfaced, recovered job not surfaced, empty DB not stale, API 200); `infra::scheduler::tests::{record_run_keeps_history_latest_first, run_history_is_pruned_to_the_limit_per_job, list_jobs_exposes_run_history, migration_0012_preserves_the_old_single_row_records}`; `web::tests::{health_banner_ui_present, jobs_ui_present}`
- [x] Docs: SCHEMA.md for the run-history shape; API.md for the new endpoint(s) — SCHEMA.md `job_runs` table (append + prune semantics) and its Relationships paragraph; API.md Health report section, the `GET /jobs` `runs` field, the web-frontend **Health banner** note and Jobs-screen history wording; README "Job and data-freshness monitoring" feature bullet

## Frontend: executed tests for the pure JS helpers + CI smoke check
~3,100 lines of JS include hand-rolled BigInt decimal arithmetic (`roundDecimalStr`,
half-away-from-zero rounding, `decStrEq` in `web/util.js`) and the allocation editor — money-adjacent
logic — yet the UI test strategy only asserts strings appear in the served bundle;
`scripts/ui-check.sh` is manual-only.
- [x] Unit tests for the pure helpers in `util.js` (rounding, thousands grouping, min-dp padding, decimal equality, numericDisplay kinds) runnable with `node --test` and no build step; include edge cases (negative values, dp increase/decrease, carry on round-up, zero) — `src/web/util.test.js` (23 tests, also covering the exact income-form arithmetic `addDecimalStrings`/`decParts`/`mulToCents`/`frankingCreditFor`/`decEq` plus `looksNumeric`/`columnKinds`/`columnLabel`); `roundDecimalStr`/`groupThousands`/`padMinDp`/`decStrEq` are now exported from `util.js`; `src/web/package.json` (`"type": "module"`) makes Node parse the modules exactly as the browser does. Writing the negative-value edge cases exposed a real bug: `addDecimalStrings` rendered a result in (-1, 0) malformed (`"-0.5" + "0.2"` → `"-.3"` — the sign broke the `padStart`), fixed by padding/splitting on the magnitude; unreachable from the current non-negative call sites, but it is exported money arithmetic
- [x] Run the JS unit tests in CI (one extra ci.yml step; document the required Node version) — ci.yml: `actions/setup-node@v4` pinned to Node 22 + `node --test 'src/web/*.test.js'` (the quoted glob needs Node ≥ 21; a bare directory argument trips over the new `package.json`); README "Tests" section documents the command and **Node 22 or newer**; pinned by `doc_checks::frontend_tests_run_in_ci`
- [x] CI smoke test via `scripts/ui-check.sh` (or an equivalent headless check): server starts on a temp DB, key hash routes render without JS errors — catches a broken module route or load-time exception that string-presence tests can't — `scripts/ui-smoke.sh` drives ui-check.sh (ephemeral server on a temp DB, `demo` fixture, headless Chrome `--dump-dom`) over four network-free routes (`#/e/trades`, `#/e/income`, `#/r/open-parcels`, `#/r/tax-summary`) and asserts per-route markers that only appear when the SPA booted and drew seeded data through the JSON API — a broken `/static` module route or load-time exception leaves the app mount empty and fails every marker. ci.yml runs it after `cargo test` on the runner's preinstalled Chrome (under `CI` the script adds `--no-sandbox` via the `CHROME_FLAGS` env override ui-check.sh gained). Verified passing end-to-end locally; the ci.yml step is pinned by `doc_checks::frontend_tests_run_in_ci`
- [x] Decide and record how the JS test files are excluded from the served-bundle route table (they must not become servable modules) — decision: test files live beside the modules as `src/web/*.test.js`; exclusion needs no mechanism because `JS_MODULES` in `web.rs` is an explicit allowlist (nothing under `src/web/` is servable unless listed), and `web::tests::js_test_files_are_not_served_and_every_module_is` pins the partition in both directions — a `*.test.js` file appearing on the route table fails, and a non-test `.js` module missing from it fails. Recorded in the `web.rs` module doc and CLAUDE.md

## Top menu bar navigation and an overview-first home screen (REQUIREMENTS 2026-07-25)
Replace the flat left sidebar with a hover-expanding top menu bar (Activity / Reports /
Reference Data / Jobs, Reports as a grouped mega-menu), make `#/` a real home route
rendering the Portfolio Overview directly, add New trade/income/sell/transfer shortcut
buttons to the overview, and reflow the overview so headline figures sit above the fold.
See REQUIREMENTS.md 2026-07-25 for full context.
- [x] New `src/web/nav.js` module: pure `navModel(entities, reports, menus)` plus
      `buildNav()`/`setActiveNav()`, replacing the sidebar code in `app.js`; add its
      `JS_MODULES` entry in `src/web.rs` (array size 5 → 6) — `web::tests::every_module_import_is_served`,
      `js_test_files_are_not_served_and_every_module_is`, `nav.test.js`
- [x] `src/web/config.js`: rename ENTITIES' `group` to `menu` (values: Activity,
      Reference Data, Jobs — Jobs gains Closing Prices, Snapshots, Row History), add
      `menu`/`section` to REPORTS, add `MENUS` export, add `shortcuts` to the `overview`
      report entry — `nav.test.js`, `web::tests::top_menu_bar_ui_present`
- [x] `src/web/index.html` + `src/web/style.css`: drop the `#layout`/sidebar markup and
      CSS, add the menu bar (`.menubar`/`.menu`/`.menu-panel`/`.menu-section`) with
      CSS-driven hover/focus-within expansion, add `.price-form` for the demoted price
      override control — `web::tests::index_is_served_as_html`, `top_menu_bar_ui_present`
      (pins the `:hover`/`:focus-within` CSS rules)
- [x] `src/web/app.js`: empty-hash route renders the overview report directly (no
      `location.hash` redirect); `viewReport` renders `report.shortcuts` as a toolbar;
      split `renderPeriodSummary` into headline stats vs. detail so `performancePanel`
      can place the chart+range control between them; demote the price-override form off
      `.card` styling with a preceding "Holdings" heading — `web::tests::overview_is_the_home_screen`,
      `portfolio_overview_ui_present`
- [x] `src/web/nav.test.js` (new): every entity/report appears in exactly one menu/section,
      menu and section ordering, href/data-key correctness including `custom` overrides — 6 tests,
      all passing
- [x] `src/web.rs`: `top_menu_bar_ui_present`, `overview_is_the_home_screen`, strengthened
      `portfolio_overview_ui_present` with the shortcuts + reflow markers
- [x] `scripts/ui-smoke.sh`: added a `#/` route check (`<h2>Portfolio Overview</h2>`, `New trade`,
      `Reference Data`)
- [x] Docs: `docs/API.md` Web frontend section (menu bar, four menus, `#/` home route, shortcuts,
      `/static/nav.js` + `/static/chart.js` module rows), README (Portfolio overview + Web UI
      bullets) — `doc_checks::top_menu_bar_documented`
- [x] `cargo build`, `cargo test` (1131 passed), `cargo fmt --check`, `cargo deny check advisories`,
  `node --test 'src/web/*.test.js'` (46 passed), `scripts/ui-smoke.sh` all clean (the pre-existing
  Chrome watchdog timeout noise in this sandbox environment affects every route uniformly, not just
  the new ones, and the DOM markers still matched before the timeout fired); a manual
  `scripts/ui-check.sh --seed demo --screenshot` pass against `#/`, `#/e/trades` confirmed the menu
  bar renders and routing still works, and a temporary `!important` CSS override (reverted before
  commit) force-opened every panel simultaneously to visually confirm the Reports mega-menu's four
  titled columns and the Activity/Reference Data/Jobs single-column panels all render their
  configured items correctly (the overlap visible in that forced-open screenshot is an artifact of
  four panels being open at once, which real hover/focus-within usage never produces)

## Health banner painted an empty strip instead of collapsing when hidden (2026-07-25 fix)
Found while screenshotting the top-menu-bar redesign above: `#health-banner`'s own `display: flex`
rule (id selector, specificity (1,0,0)) outranks the browser's built-in `[hidden] { display: none }`
rule (attribute selector, specificity (0,1,0)), so the `hidden` attribute alone never collapsed the
banner — it painted an empty coloured flex row (background + padding, no content) on every screen
whenever nothing needed attention, pre-dating and unrelated to the menu-bar work.
- [x] `#health-banner[hidden] { display: none; }` added to `style.css` — an id+attribute selector
      (specificity (1,1,0)) beats the plain id rule regardless of source order, so it collapses
      correctly however the two rules are ordered
- [x] Test: `web::tests::health_banner_ui_present` now pins the exact override rule's presence in
      the served stylesheet
- [x] `cargo build`, `cargo test` (1131 passed), `cargo fmt --check`, `cargo deny check advisories`,
  `node --test 'src/web/*.test.js'` (46 passed) all clean; `scripts/ui-check.sh --seed demo
  --screenshot '#/'` confirmed the stray strip is gone

## Top menu bar contrast and narrow performance panel (2026-07-25 fixes)
User-reported after the top-menu-bar redesign above: a hovered/active top-level menu label
(`.menu-label:hover, .menu-label.active { color: #fff; }`) read as almost-white text on an
almost-white background, and the performance panel's chart and stat grid were capped at the
`.card` default's form-friendly 720px even though the removed sidebar freed up much more width.
- [x] Root cause of the contrast bug: the generic `button:hover { background: #eef1f5; }` rule sets
      a *different* property than `.menu-label:hover`'s `color: #fff`, so it still applied on top —
      near-white text landed on a near-white button. Fixed by giving `.menu-label:hover`/`.active`
      their own explicit `background: rgba(255, 255, 255, 0.12)` (a light overlay on the dark
      topbar, visible on hover and when the current screen's menu is highlighted) — the higher
      specificity of the combined selector wins outright rather than relying on property ordering.
      Test: `web::tests::top_menu_bar_ui_present` pins the fixed rule
- [x] Widened the performance panel: gave the panel a `perf-panel` class (both the `<2`-snapshot
      degenerate card and the full chart+stats card in `performancePanel()`, `app.js`) with its own
      `max-width: 1200px` (`.card`'s 720px default stays for data-entry forms, which shouldn't
      stretch), and raised `.series-chart`'s cap from 900px to 1160px (used nowhere but this panel,
      so no other view is affected) — the `.perf-summary` stat grid already reflows via
      `auto-fit`/`minmax`, so it fills the extra width automatically
- [x] `cargo build`, `cargo test` (1131 passed), `cargo fmt --check`, `node --test
  'src/web/*.test.js'` (46 passed) all clean; `scripts/ui-check.sh --seed demo --screenshot '#/'`
  confirmed the active Reports label is legible and the panel now spans most of the content area

## Manual Price Overrides moved below the holdings table (2026-07-25 follow-up)
User feedback on the reflowed overview: the Manual Price Overrides control and its Run report
button (the shared `viewReport` price-form branch, `app.js` — used by every prices-with-no-params
report: Portfolio Overview, Unrealised Gains, Performance) still appeared between the "Holdings"
heading and the table, ahead of the data it overrides. Reordered `setMain`'s children so the
result (as-at line + table) renders directly under the "Holdings" heading, with the price form
last.
- [x] `app.js`: swapped `[…, priceForm, result]` to `[…, result, priceForm]` in the shared
      price-form branch; updated the branch's comment (it now sits below, not "beside", the table)
- [x] `cargo build`, `cargo test` (1131 passed), `cargo fmt --check`, `node --test
  'src/web/*.test.js'` (46 passed) all clean; `scripts/ui-check.sh --seed demo --screenshot '#/'`
  confirmed Manual Price Overrides and Run report now render below the holdings table


## Listing Activity laid out like the Portfolio Overview (2026-08-28 user request)
"The listing summary screen should be similar layout to the Portfolio overview screen. First have
the chart, then the holdings summary followed by all the listing activity in the same order it is
now" — and, in a follow-up the same request: "the listing selector should be at the top, with the
price override collapsed under the holdings summary like in portfolio overview". The screen led
with its whole params form (listing + price) and then the ledger, so its two headline facts — what
the holding is worth and how it got there — were the last things on the page, and it had no graph
at all.

Both halves were done generically rather than as a bespoke view: the graph and the split form are
config fields on the report (`chart` / `paramsPanel`), so another params report can take the same
shape. The chart is the overview's own, narrowed server-side.
- [x] `reports::snapshot`: `db_series` takes `Option<i64>`, surfaced as `GET
      /report_snapshots/series?listing_id=`. A narrowed series sums that listing's unrealised-gains
      rows alone; a date it has no row on is **not a point** (it was not held then, or was held and
      excluded — a subset sum cannot report an exclusion as a stepped total the way the portfolio
      series does, so the gap in the line is the signal), and `provisional`/`price_carried_forward`
      come from that listing's rows rather than the snapshot's portfolio-wide flags. `stale` stays
      the snapshot's own. Test: `api_the_series_narrows_to_one_listing`
- [x] `app.js`: the chart, its series selector and its range control extracted out of
      `performancePanel` into `rangedChart`, which both screens now build from — no second copy of
      the preset/ResizeObserver/redraw machinery. `listingChartPanel` is the new caller, with its
      own remembered range and series keys (`share-tracker.activity.*`): one screen reads the whole
      portfolio and the other a single holding, so a shared key would have the last screen visited
      silently re-range the other
- [x] `viewReport`: `report.chart` names the param the graph is scoped to (drawn above the tables,
      filled when its own request lands rather than holding the tables back); `report.paramsPanel`
      moves named params out of the header form into a collapsed panel re-appended under the table
      it belongs beneath. Both forms submit one body — which form a field is read from is decided
      once, in `fieldOwner`
- [x] `config.js`: `chart: 'listing_id'`, `paramsPanel: { fields: ['price'], after: 'holdings', … }`,
      and the two tables swapped so the holding summary is above the ledger (whose own newest-first
      order is untouched)
- [x] Docs: `docs/API.md` (the series endpoint's filter + the screen's layout under Listing
      activity), `docs/FEATURES.md`
- [x] `cargo test` (2345 passed), `cargo fmt --check`, `cargo clippy --all-targets -D warnings`,
      `node --test 'src/web/*.test.js'` (126 passed) all clean. `scripts/ui-smoke.sh` gained the
      deep-linked `#/r/activity/1/12.50` route (the price is passed so the summary does not value
      live — no smoke route may touch the network). Driven end to end with `scripts/ui-drive.js`
      against the showcase fixture: switching listing in the top selector re-scopes the graph (BHP
      axis 7,310–8,439 → VAS 42,863–44,365), the series selector and range presets work on it, and
      the collapsed override re-values the summary with the layout order intact across re-renders


## SCENARIOS R-01/R-05: the rename feature has no web UI, and the listing form sends the user to an endpoint the UI does not offer

`POST /listings/:id/rename`, `GET /listings/:id/renames` and `DELETE /listings/:id/renames/:id` have
no screen. `config.js` mentions `listing_renames` in exactly one place — the Row History table
picker — and `ACTIONS` has no rename entry, so from the web UI a rename cannot be recorded, the
chain cannot be read, and an undo cannot be run.

The gap is self-announcing: editing a ticker on the Listings form for a listing with any recorded
trade answers `422` — "use POST /listings/:id/rename to record a ticker or exchange change on a
listing with recorded trades, income, or prices" — which the toast shows verbatim. The UI's own
error text names an HTTP endpoint the UI never calls.

Per CLAUDE.md this is the shape `ACTIONS` exists for: an owner-row action rendered by the generic
`viewAction`, like reinvest/exercise/participate/demerge.

- [x] Add a `rename` entry to `ACTIONS` in `config.js` — owner `/listings`, fields
      `effective_date`, `ticker`, `exchange_mic`, `name`, `price_symbol`, `note` — so the refusal's
      remedy is reachable from the screen that raises it.
- [x] Show the chain: a rename history view over `GET /listings/:id/renames` with the newest entry
      undoable, or the chain rendered on the listing's own row. Decide which rather than building
      both.
- [x] Update the `docs/API.md` Web frontend paragraph, which lists the UI's screens and actions, once
      the entry exists.

**Closed 2026-08-21.** One `ACTIONS` entry plus one owner-scoped child-collection view, no new
generic machinery:

- `config.js`: a `rename` entry in `ACTIONS` (owner `/listings`, `post` →
  `POST /listings/:id/rename`, `cancel` `#/e/listings`, submit "Rename") with the six fields the
  endpoint takes — `effective_date` and `ticker` required, `exchange_mic` (an `exchanges` `fk`),
  `name`, `price_symbol` and `note` optional, each hint naming the listing's *current* value so
  "blank keeps it" is checkable on the screen. Its `desc` states every rule the API enforces — the
  no-op refusal, both `effective_date` bounds, the ticker collision, the exchange-currency
  boundary, the two Crypto rules, and that a takeover relisting is a `ScripForScrip` action, not a
  rename — so the screen promises nothing the endpoint refuses. Rendered by the generic
  `viewAction`, which already omits blank optional fields from the body, exactly the "omitted keeps
  it" contract the endpoint documents.
- The Listings rows carry two link `rowActions`, **Rename** (`#/rename/:id`) and **Rename history**
  (`#/renames/:id`), so the remedy the `PUT` refusal names is one click from the row that raises
  it; the listings `desc` now says a rename is not a field edit and points at both.
- **Decision on the second item — a rename-history view, not the chain on the listing's row.** The
  chain has five before/after columns plus a note, and the undo is a per-row mutation: rendering
  that inside a listings-table cell would need new expand/mutate machinery in `filterableTable`
  (which no other screen has) and would still have nowhere sane to put the confirm. The app already
  has exactly this shape — the per-record **Attachments** view: an owner-scoped child collection on
  its own hash route, listed through the shared `filterableTable`, with the mutation the API allows
  offered per row. `viewListingRenames` (`app.js`, `#/renames/:listing_id`) copies it: reads
  `GET /listings/:id/renames`, renders the chain newest-first through `filterableTable`, and offers
  **Undo** (`DELETE /listings/:id/renames/:rename_id`) on the newest entry alone — matched by row
  *identity*, not position, so a re-sort keeps it with the right row. Offering it on every row would
  only produce the 422 the API answers for a non-newest undo. The view also links back to
  `#/rename/:id` ("+ Record a rename") and to Listings, and reads "No ticker or exchange changes
  recorded." for a listing with no chain.
- `util.js`: `old_exchange_mic`/`new_exchange_mic` added to `COLUMN_LABELS` as "Old exchange"/"New
  exchange", keeping them in step with `exchange_mic` → "Exchange" rather than humanising to "Old
  exchange MIC".
- `docs/API.md`: the Web frontend paragraph now carries the Rename action (naming the `PUT` refusal
  it answers), the Rename history view and its `GET`, and the newest-only Undo with its
  last-in-first-out reason; `#/renames/<listing>` added to the hash-route list.
- Tests: `web::tests::listing_rename_action_ui_present` (the action entry, its POST path, the row
  link, all six fields with their required/optional flags, and each refusal rule stated on the
  screen) and `web::tests::listing_rename_history_ui_present` (the view, its route, both API paths,
  the `filterableTable` render, the chain columns, and the newest-only undo by identity), plus
  `doc_checks::listing_rename_ui_documented` pinning the Web frontend paragraph's new text.
- `cargo build`, `cargo test` (1857 passed), `cargo fmt --check`, `cargo clippy --all-targets -D
  warnings` all clean; `node --test 'src/web/*.test.js'` 69 passed. End-to-end with
  `scripts/ui-check.sh` against a temp DB seeded from the demo fixture plus two recorded renames:
  `#/e/listings` shows Rename / Rename history on every row, `#/renames/1` renders both chain rows
  (VAS→VASX→VASY) with the right headings and **Undo on the newest row only**, `#/renames/2` (no
  chain) renders the empty-state line, and `#/rename/1` renders the form with all six inputs, the
  exchange picker populated, and the hints quoting the listing's current ticker, name and exchange.

## SCENARIOS Y-a — an error toast holds a refusal for six seconds and then it is gone

- [x] Make an error toast persist until it is dismissed, and announce it. Option 1, as chosen.
      `util.js`'s `toast()` is now two tiers that differ in *kind*: a success toast keeps its 3 s
      auto-hide (measured 3,043 ms after, 3,051 ms before — unchanged), an error toast is given no
      timeout at all and stays until dismissed (measured still on screen, text intact, at 15,042 ms;
      before it hid at 6,595 ms from the click). Dismissal is a real `<button class="toast-close">`
      (click, Enter and Space all measured), click anywhere on the toast, and Escape while the close
      button has focus. The item carries `role="alert"` (error) / `role="status"` (success) — on the
      inserted item rather than on `#toast`, which is `hidden` whenever the stack is empty and so is
      not a live region anything is already watching. Signature unchanged, so none of the ~25 call
      sites moved.
      **Replacement:** toasts now *stack* instead of overwriting. Silently replacing an undismissed
      error loses exactly what the auto-hide lost, so a new message is appended below; the newest is
      the bottom item, where the single toast used to sit. A repeat of a message already showing
      re-inserts that one (so it is announced again) rather than stacking a duplicate — driven:
      four toasts, one a repeat, leave three items with the first error still readable. The stack is
      capped at the viewport and scrolls, so nothing is ever dropped to make room (five undismissed
      251-character errors fit inside 820×700).
      **Navigation:** clears nothing. An error naming the records to go and remove is read *while*
      navigating to remove them, so clearing on the hash change would destroy it at the moment it is
      being acted on — and several save paths set `location.hash` immediately after toasting, so
      clearing on navigation would mean a "Saved." was never seen at all. Driven: the refusal
      survives two hash changes. `web::tests` pins this the strong way, by asserting the `#toast`
      element is reached from exactly one place in the whole served bundle — `toast()` itself — so
      no view and no route change *can* take a refusal away.
      One thing the write-up did not raise, found while measuring: last in the document, the close
      button took **113 Tab presses** to reach on the listings screen. `#toast` is now first in
      `<body>` (`position: fixed`, so the layout is identical) — **1 Tab press**.
      Rendering is unchanged at all three viewports the finding measured (2/2/3 lines at 1280×900,
      1024×768, 820×700, nothing clipped, no horizontal body scroll), a 1,059-character refusal
      wraps to 7/10/12 lines and still fits, and `@media print` still hides it.
      Tests: `toastLifetime` unit-tested in `src/web/util.test.js` (an error's lifetime is
      *absent*, not merely longer); `web::tests`'s
      `error_toasts_persist_until_dismissed_and_announce_themselves` and
      `toast_stack_markup_and_styling` pin the wiring, the single-owner negative assertion above,
      the tab-order position and the print/hidden CSS rules. `docs/API.md`'s "Error bodies"
      paragraph now says the error toast stays until dismissed and that toasts stack.

Driven through `scripts/ui-drive.js` against a throwaway database. Deleting a listing that nine other
tables draw on answers `422 this listing is still referenced by AMMA statements (1), closing prices
(1), corporate actions (1), DRP enrolment periods (1), ESS statements (1), income (1), inheritances
(1), investment expenses (1), trades (2) — remove those records first`. The toast **does** show it in
full — measured at 1280×900, 1024×768 and 820×700, wrapping to 2–3 lines, never clipped, never
overflowing the viewport, `scrollHeight == clientHeight` — so the rendering half of Y-02 is correct.

What is not correct is how long it is there for. Measured: **6,045 ms** from click to `hidden`, fixed
(`util.js`'s `toast()` uses `isError ? 6000 : 3000`). It has no close button, no click handler
(clicking it does nothing), no `role` and no `aria-live`, and once it hides the text exists **nowhere
else in the document** — checked. So the user has six seconds to read and memorise nine table names
before the only statement of why their delete failed is destroyed, with no way to bring it back. The
longest 422 the API can produce is a serde unknown-field rejection at **638 characters**; it renders
in three lines and is equally unrecoverable.

The same six-second window produced a false finding inside this very pass: a "Saved." toast still on
screen from the *previous* entity was read as `tax_year_settings` having saved a blank form. A toast
that outlives the action it belongs to is ambiguous in both directions.

**Options offered:**
1. **Error toasts persist until dismissed** — an error toast stays up with a close button (and
   click-to-dismiss); success toasts keep the 3 s auto-hide. Add `role="alert"`.
2. Scale the timeout to the message length (e.g. 6 s + 60 ms/char, capped), plus `role="alert"`.
3. Keep the toast and additionally render the refusal into a persistent inline block in the view.
4. Leave it and record a known limitation.

**Chosen: option 1.** No arithmetic, matches the existing two-tier success/error split, and the
message stays until the user has actually dealt with it.

## SCENARIOS Y-b — a 50-parcel allocation is refused without saying what it adds up to

- [x] Name the sum in the refusal, and show a running total in the allocation editor. Both halves
      done. Server: `SellError::AllocationMismatch` now carries `{ allocated, quantity }` and its
      `From<SellError> for ApiError` arm answers `the allocations sum to {allocated}, not the
      {quantity} units sold` — the exact wording `reports::net_capital_gain` already uses on the
      what-if path, and it reads correctly for the buy-back participation that raises the same
      error (its own body field is `units`). `rights_sale`'s equivalent split in two, because a
      negative row can sum correctly while being nonsense: `AllocationsDontSum { allocated, units }`
      → `the allocations sum to {allocated}, not the {units} rights sold` (an empty list falls out
      here as a nil sum), and a new `AllocationNotPositive` → `each anchoring parcel allocation must
      be for a positive number of rights`. `transfer` confirmed to have no sum invariant — its
      Sell's quantity *is* the allocations' own sum. Client: `allocationEditor` renders a live
      running total over a new pure `util.js` `allocationSummary()` (exact decimal-string
      arithmetic via `addDecimalStrings`, never parseFloat — 50 eight-decimal crypto rows are unit
      tested), muted while nothing is allocated yet, green on a match, amber on a shortfall/excess,
      and it names the amount. Callers: the Sell form passes `requiredTotal` reading its
      `quantity`; the two allocation-taking actions declare `requiredField: 'units'` in `config.js`
      and `viewAction` turns that into the same closure; the Transfer's two editors deliberately
      declare none (no target exists) and show the running total alone. Driven at 50 rows: before,
      `HTTP 422: the parcel allocations do not sum to the sell quantity` with no total on screen;
      after, `Allocated: 4910 of 5000 — 90 short.` live in the editor and `HTTP 422: the
      allocations sum to 4910, not the 5000 units sold` from the server. Tests: rejection-wording
      tests in `sell.rs`/`rights_sale.rs`/`buyback_participation.rs`, 9 `util.test.js` cases,
      `web::tests::allocation_editors_show_a_running_total`; `docs/API.md` updated in three places.
      NOTE: the reproduction DB no longer held the finding's 50 open parcels — an earlier session's
      successful 5000-unit Sell (trade 204) had consumed them; deleted it to restore the state.

Y-03 driven with 50 open parcels on one listing. The editor itself holds up: "+ Add allocation" 49
times took 413 ms, all 50 rows render, each parcel select carries all 50 options labelled
`101: FIFTY — 100 remaining (acquired 2022-01-10)`, the submit button stays reachable, and a correct
50-row allocation saves (`Sell saved.`). A wrong one is refused safely.

But with one row typed as `10` instead of `100`, the refusal is
`HTTP 422: the parcel allocations do not sum to the sell quantity` — it never says what they *do* sum
to, no row is marked, and the editor shows no running total (`allocationEditor` renders heading,
hint, rows and an add button, and nothing else). So the user is told 50 numbers are wrong by an
unstated amount and must add them up by hand to find out which — inside the six-second window of
Y-a.

The better wording already exists one room away: `reports/net_capital_gain.rs:1244` refuses the same
condition on the what-if path as `the allocations sum to {total}, not the {n} units sold`. The write
path that matters has the worse message. Scope is `entities/sell.rs`'s `SellError::AllocationMismatch`
(shared with buy-back participation) and `entities/rights_sale.rs`'s equivalent; `transfer` moves
whole parcels and has no sum invariant.

**Options offered:**
1. **Both** — give the 422 the figures, and add a live allocated-vs-required total to the shared
   `allocationEditor`.
2. Server message only.
3. Running total in the editor only.
4. Leave it.

**Chosen: option 1.** The server half is the correctness fix and is testable; the editor half is what
actually helps at 50 rows.

## SCENARIOS Y-c — the AMIT confirm dialog's own numbers do not add up

- [x] Return each generated adjustment's re-based quantity too, and show both in the confirm gate.

Y-05 driven on an AMMA statement over a listing carrying a 1-for-2 `ShareSplit` between acquisition
and the statement's year end. The gate works in every other respect: it previews without writing,
Cancel writes nothing (0 adjustments after), Accept writes 2 and reports the mismatch in the toast,
and the mismatch warning is prominent. What it showed was:

```
  • 10: Buy 1000 (XASX:MEGA, 2023-08-10) — 1000
  • 11: Buy 5 (XASX:MEGA, 2024-05-01) — 5

Adjusted units 2005 vs the statement’s units held 1000

⚠ MISMATCH of 1005 units.
```

The listed quantities sum to 1,005; the stated total is 2,005; and the mismatch figure is *also*
1,005, which actively invites the reader to think one of the two is a typo for the other. Both server
figures are right and deliberately so — `GeneratedAdjustments` documents that `created[].quantity` is
each parcel's **as-acquired** units while `units_adjusted` is "re-based into the statement year's unit
basis, so it is comparable with `units_held`". The dialog puts the two bases side by side unlabelled.

**Control:** the same generation over a listing with no corporate action lists 50 parcels summing to
exactly `units_adjusted` (5,000 = 5,000). The split re-basing is the whole cause — nothing else.

**Options offered:**
1. **Return both bases per row** — `created` gains the re-based quantity, so the dialog can read
   `1000 units (2000 in the statement year's basis)` and the list visibly reaches the total.
2. Label the difference in the dialog text only (no API change).
3. List the re-based quantities instead of the stored ones.
4. Leave it.

**Chosen: option 1.** Option 3 was rejected because the dialog would then show figures that do not
match the AMIT Adjustments screen afterwards; the stored quantity must stay visible.

**Done.** `created` is now `Vec<GeneratedAdjustment>` — a wrapper carrying the stored `AmitAdjustment`
`#[serde(flatten)]`ed (so the JSON row is unchanged: `id`, `amma_statement_id`, `trade_id`,
`quantity`) plus its own `units_adjusted`, that row's quantity re-based into the statement year's
basis. The figure was already being computed and summed at both push sites and then discarded; it is
now attached to the row, so `Σ created[].units_adjusted == units_adjusted` holds by construction and
a test asserts it. `adjustmentPreviewText` shows the second figure only on rows where the two differ,
plus one explanatory line when any row does — the no-split control's dialog is byte-identical to
before (diffed). The split case now reads
`• 10: Buy 1000 (XASX:MEGA, 2023-08-10) — 1000 (2000 in the statement year’s basis)` above
`• 11: … — 5`, so the list visibly reaches 2,005. Tests:
`db_a_split_across_covered_parcels_is_costed_on_the_year_end_basis` pins both per-row bases and the
Σ invariant, `db_a_split_after_the_year_end_does_not_change_the_reduction` pins the equal case,
`api_generate_returns_201_with_the_created_rows` pins the flattened wire shape, three
`adjustmentPreviewText` cases in `util.test.js` pin the wording (including "equal but written with a
different number of decimals shows no bracket"), and `doc_checks` pins the `docs/API.md` paragraph.
Nothing else consumed `created[]` — `forms.js`, `config.js`'s toast and `taxreport.js` read only the
top-level totals (taxreport.js's `units_adjusted` is the cross-check report's, a different response).

One correction to the write-up above: the finding says "the mismatch figure is *also* 1,005", reading
as if the mismatch coincidentally equalled the listed sum. It does, but by arithmetic accident of
this fixture (1000 held, 2005 adjusted), not by any shared derivation — `difference` is
`units_adjusted − units_held` and never touches the listed quantities. The confusion the fix removes
is the unlabelled two bases; the coincidence is fixture noise.

## SCENARIOS Y-d — two Tax Summary money columns are rendered at four decimal places, ungrouped

- [x] Classify both in `COLUMN_KINDS`, and pin the list with a test so the next one cannot drift.

Y-10 driven by sweeping every numeric column actually rendered on every entity list and every report
against `util.js`'s `columnKinds()`. The entity lists came back clean — every unclassified numeric
column there is an id, a foreign key, or a code that verbatim rendering is *required* for
(`currencies.numeric_code` is `036`, which money formatting would destroy). The performance report's
`total_return_pct` / `money_weighted_return_pct` are unclassified but the server already
`round_dp(4)`s both, so verbatim is stable and correct.

Two real gaps, both in the Tax Summary and both `Decimal` money on the server
(`reports/tax_summary.rs:103,135`): **`employment_income`** and
**`foreign_tax_offsets_cgt_discount_reduction`** are absent from `COLUMN_KINDS`' money list. With an
employment-income row of 1,234,567.8912 entered, the same figure renders three different ways:

| where | rendered |
| --- | --- |
| Tax Summary screen, `Employment income` | `1234567.8912` |
| Tax Summary screen, neighbouring `Gross assessable investment income` | `2,757.30` |
| the screen's own **Export CSV** | `1234567.89` |

So the screen is the odd one out, and it is odd against its own export — the CSV was already brought
to cents by W-c/W-f. This is that same rule with a hole in it, in the one place a hand-maintained
name list can always grow one.

**Options offered:**
1. **Add both, plus a test pinning the list** — walk every report/entity payload and require each
   `Decimal` column to be in `COLUMN_KINDS` or an explicit verbatim allowlist.
2. Just add the two columns.
3. Leave it.

**Chosen: option 1** — W's "when a rule needs a list, move the list somewhere the compiler sees it",
applied to the UI's copy of it.

**Done.** Both are classified `money` in `util.js` (the screen now reads `1,234,567.89` with
`1234567.8912` on hover, matching its own CSV export), and
`web::tests::every_money_column_on_the_wire_has_a_display_kind` pins the map: it walks every
`Decimal` / `Option<Decimal>` field of every `Serialize` struct **and enum** under `src` — the
server's whole wire surface — and requires each to be classified in `COLUMN_KINDS` or listed with
its reason in one of two verbatim allowlists: `UNCLASSIFIED_WIRE_DECIMALS` (server-rounded
percentages, the FX-import conflict rows, the chart's series point, the tax report's per-unit
adjustment detail) or `NOT_RENDERED_THROUGH_COLUMN_KINDS` (the Annual Tax Report and its
`CgtSummaryYear`, which `taxreport.js` formats itself). A stale or now-classified allowlist entry
fails too. It keys off the Rust *type*, which is why ids, foreign keys and `numeric_code` (`036`)
never enter the walk at all. Proven with three mutations (drop `employment_income`; add a money
field to a report payload; classify an allowlisted column) — each fails, each restore passes. The
`src` walk itself moved to `test_support::rust_sources`, shared with `infra::db`'s deferred-`BEGIN`
scan rather than copied.

Two corrections to the write-up above. The Y-10 sweep's "entity lists came back clean" holds only
for the columns a *populated* fixture row happened to render — which is exactly the hole a
type-driven walk closes. Two more unclassified `Decimal`s were sitting on the Corporate Actions
list's own column list (`scrip_cash_per_unit`, `scrip_market_value`; both per-unit figures, so
classifying them `rate` changes no pixel — but they were unclassified all the same), and eight more
on payloads no fixture row put on screen: the trade DRP residual trio, `difference`, and the health
report's four figures. All classified now. And the health report's `cost_base_total`,
`discount_total`, `statement_discount`, `units_sold` are classified now but still print raw, because
the health banner (`app.js`) concatenates them into prose without consulting `COLUMN_KINDS` — as
does the AMIT generate-adjustments confirm dialog for `difference`. Money in prose is a second,
separate hole in the same rule (`taxreport.js` solved it with a `moneyText` helper); worth its own
finding. Also noticed: `buy_brokerage`, `buy_gst_on_brokerage`, `cost_base_per_unit_aud`,
`proceeds_per_unit_aud` and `tfn_withholding_aud` are on the Annual Tax Report's wire but named
nowhere in `taxreport.js`.

## SCENARIOS Y-f — `#/e/<custom-slug>` renders a raw JavaScript TypeError as the whole page

- [x] Send `#/e/jobs`, `#/e/closing_prices`, `#/e/sells` and `#/e/transfers` to their real routes. One line in the `parts[0] === 'e'` branch: an entity with a `custom` view is sent on with `location.replace('#/' + entity.custom)` — `replace`, not an assignment to `location.hash`, so the dead URL is not left in history for Back; the hashchange it fires re-enters `render()`, which resolves the custom route and stops. Placed before the `/new` and `/edit` checks, which were broken for these four too (`#/e/sells/new` → `entity.keyFields is not iterable`, `#/e/sells/edit/1` → `HTTP 405`) and now land on the real list screen. Driven in headless Chrome: all four (and a cold load straight onto `#/e/jobs`) land on `#/jobs` / `#/prices` / `#/sells` / `#/transfers` with the right `h2` and no console error, Back from a redirect returns to the route before it, all 21 non-custom entities still render at `#/e/<slug>`, and `#/e/nonsense` still renders "Unknown view". Pinned by `web::tests::custom_view_entities_redirect_off_the_generic_entity_route`, which asserts the `location.replace` form and that every `custom:` slug in config.js's ENTITIES has a router branch to land on; `docs/API.md`'s hash-route sentence notes the redirect.

Four entities are rendered by custom views reached at their own routes (`#/jobs`, `#/prices`,
`#/sells`, `#/transfers`). `app.js`'s router still resolves `#/e/<slug>` for them through the generic
`viewEntityList`, which then fails: `#/e/jobs` → `Cannot read properties of undefined (reading
'map')`, `#/e/closing_prices` and `#/e/transfers` → `... (reading 'concat')`, `#/e/sells` →
`HTTP 404`, each rendered as the entire page body.

Reachability is the mitigating half and was checked rather than assumed: `nav.js` correctly links
`'#/' + e.custom` for a custom entity, no other module builds an `#/e/<custom>` href, and the 98-route
sweep of Y-09 found no other broken route — every route the app itself generates renders a view. So
this is reachable only by a hand-typed or stale-bookmarked URL, which is why it is logged last.

**Options offered:**
1. **Redirect to the entity's real route.**
2. Reject a custom entity in the generic branch so it renders the existing "Unknown view".
3. Dismiss as unreachable.

**Chosen: option 1.** One line in the router, and a stale bookmark lands somewhere useful rather than
on what reads as a crash.

## SCENARIOS Y-g — the health banner prints money as prose, unformatted

- [x] Format the six banner figures through the shared money formatter, and pin the convention.
  `moneyText` — the wrapper `taxreport.js` already had for exactly this — moved into `util.js` as
  the one shared prose-money formatter (`numericDisplay(v, 'money')`, falling back to `cellText`);
  `taxreport.js` now imports it instead of declaring its own, and `app.js`'s six `problems.push`
  sentences wrap their figure in it. Quantities beside them (`quantity`, `units_sold`) stay
  verbatim. **No hover**: the whole banner is one `<span>` holding `problems.join(' ')`, so there
  is no element per figure to hang a `title` on — and the alert names the row ids, whose own screen
  shows the full value on hover. Line numbers in the write-up above had all shifted by ~17 (Y-f);
  the six sites are now 1309 / 1316 / 1323 / 1331 / 1340 / 1362.
  Pinned by `web::tests::money_in_prose_goes_through_the_shared_formatter`: it reuses
  `column_kinds()` (the same `util.js` parser `every_money_column_on_the_wire_has_a_display_kind`
  uses) and scans every served module for a leaf read of a money column that is an operand of `+`
  beside a string literal, or interpolated into a `${…}`. `MONEY_PROSE_ALLOWED` exists but is
  **empty**: requiring the adjacent literal excludes arithmetic, and requiring a leaf read excludes
  `config.js`'s `r.income.id` — the write-up's one false positive needed no entry after all.
  A sibling test (`the_prose_money_scan_separates_sentences_from_sums`) pins the matcher itself
  against both, and each of the six was individually re-broken and re-run: all six fail the scan
  raw, none when wrapped. Driven end to end (`scripts/ui-drive.js`, throwaway DB seeded so all six
  fire at once): `12340.1234` → `12,340.12`, `1234567.8912` → `1,234,567.89`, `9876.5432` →
  `9,876.54`, `4321.9876` → `4,321.99`, `8765.4321` → `8,765.43`, with `1000 shares` / `400 …
  shares` / `5 MEGA` unchanged. `docs/API.md`'s health-banner paragraph states the rule.
  Two corrections to the write-up above: the `cost_base_total` row is the **duplicate
  inheritances** alert (`d.inheritance_count + ' identical inheritances of …'`), not
  "duplicate ESS parcels" — inheritances are where that column lives; and `config.js`'s
  match is on `r.income.id`, the id *inside* the row object, not on `r.income` itself,
  which is what made a sharper matcher able to exclude it structurally.

Found while fixing Y-d, and deliberately kept separate from it: `COLUMN_KINDS` governs table *cells*,
and the health banner does not build cells — it builds sentences, concatenating the figure straight
in. Same rule, different render path, so Y-d's fix does not reach it.

Verified live with two identical ESS statements carrying a 12,340.1234 discount:

> ⚠ … 2 identical ESS statements for MEGA vesting 1000 shares at 2024-01-15 with a **12340.1234** AUD
> discount (ids 10, 11) — …

Every table in the app renders that same figure `12,340.12` with the full value on hover.

**Bounded by a sweep of all seven JS modules** rather than by estimate — a money name from
`COLUMN_KINDS` concatenated into a string without passing through `numericDisplay`. **Six sites,
all of them `problems.push(...)` in `app.js`'s health banner:**

| line | figure |
| --- | --- |
| `app.js:1292` | `gross_amount` — duplicate income rows |
| `app.js:1299` | `amount` — duplicate interest rows |
| `app.js:1306` | `amount` — duplicate investment expenses |
| `app.js:1314` | `discount_total` — duplicate ESS statements |
| `app.js:1323` | `cost_base_total` — duplicate ESS parcels |
| `app.js:1345` | `statement_discount` — an ESS parcel sold inside 30 days |

(The one other match, `config.js:869`, is a false positive: `r.income` is a row object, not a figure.)
Quantities in the same sentences — `units_sold`, `quantity` — are correctly verbatim and stay so.

The sanctioned fix already exists in the tree: `taxreport.js` wraps `numericDisplay(v, 'money')` as
`moneyText` for exactly this case, the Annual Tax Report being the other place money is written
outside a `filterableTable` cell.

**Options offered:**
1. **Format all six, and pin the convention with a scan** — route them through the shared formatter,
   then add a test that scans the JS modules for a `COLUMN_KINDS` money name concatenated into a
   string without going through it.
2. Just format the six.
3. Leave it — the banner is prose, not a figure the user transcribes.

**Chosen: option 1.** This is the second hole found in one rule in one pass, which is the argument
for pinning the call-site convention rather than fixing six call sites and hoping.
