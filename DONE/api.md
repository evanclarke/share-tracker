# Done — HTTP / REST API Surface

## REST API audit — documentation fixes (2026-09-24)

An audit of the HTTP surface against `docs/API.md`. The code is accurate; every item here is a
`docs/API.md` correction, each closed by a `doc_checks.rs` assertion that the required text is now
present/absent.

- [x] Document `date_received` as a **required** AMMA-statement field. `src/entities/amma.rs:109`
      declares `pub date_received: NaiveDate` with no `#[serde(default)]`, so it is mandatory — yet
      `date_received` appears nowhere in `docs/API.md`. Add it to the AMMA statements section
      (docs/API.md ~531–572) as required. A machine client omitting it gets an unexplained
      `422 missing field date_received`. Test: `doc_checks` asserts the AMMA section names
      `date_received` as required.
- [x] Complete the `201 Created` enumeration in the Response codes table (docs/API.md:1800). It
      lists the operation endpoints but omits the 14 standard `POST /<collection>` creates and
      `POST /ess_statements/:id/vest` — all `201` in code — contradicting the table's own "Creating
      a record" section (docs/API.md:77). Test: `doc_checks` asserts the `201` row names the
      collection-POST create path and `vest`.
- [x] Document the `preference` listing field. `src/entities/listing.rs:116`
      (`#[serde(default)] pub preference: bool`, the 90-day preference-share flag read by franking
      at-risk) is never named in the Listings section (docs/API.md:123–191). Test: `doc_checks`
      asserts the Listings section documents `preference`.
- [x] Drop the stale `409` from the "Error bodies" list (docs/API.md:1836). No code path returns
      HTTP 409 (no `StatusCode::CONFLICT`; `ApiError` has no 409 variant), and the Response codes
      table itself has no 409 row, so the doc is self-inconsistent. Test: `doc_checks` asserts the
      error-bodies list names only the codes the code actually returns.
- [x] Document the three DRP residual columns `PUT /trades` accepts —
      `src/entities/trade/model.rs:327-332` (`residual_brought_forward`/`residual_carried_forward`/
      `residual_paid_out`, all `#[serde(default)]`) — which are referenced only indirectly
      (docs/API.md:1848), never as trade body fields. Note they are the reinvest-created DRP's
      server-managed residual chain, not client-writable in practice. Test: `doc_checks` asserts the
      Trades section names them (or says they are server-managed).

Closed 2026-09-25: all five corrections landed in `docs/API.md`, each pinned by a
`doc_checks` test — `amma_date_received_documented_as_required`,
`created_response_row_names_collection_creates_and_vest`, `listing_preference_field_documented`,
`error_bodies_list_names_only_returned_codes`, and `drp_residual_columns_documented_in_trades`
(the last two also cross-check the doc against `src/infra/http.rs`'s status map and against the
Response codes table, so the sections cannot drift apart again).

## REST API audit — consistency fixes (2026-09-24)

- [x] Fix the natural-key DELETE 404 wording. `infra::http::deleted` hard-codes
      `no {noun} with that id` (`src/infra/http.rs:156`), which is wrong for `DELETE /exchanges/{mic}`
      (key is `mic`, `src/entities/exchange.rs:123-124`) and `DELETE /tax_year_settings/{tax_year}`
      (`src/entities/tax_year_settings.rs`). Give each a key-specific body (`no exchange with that
      mic`, `no tax year settings row for that year`); `exchange_holidays` already hand-writes its
      composite-key message (`exchange_holiday.rs:252-254`). Test: extend
      `entities::tests::deleting_a_missing_row_is_404_naming_what_was_missing` to assert the
      key-specific wording.
- [x] Make `GET /rights_sales/{id}` consistent with the empty-body GET-one 404.
      `src/entities/rights_sale.rs:737` returns `no rights sale with that id`, the only GET-one that
      carries a body; every other GET-one uses the empty `ApiError::NotFound`. Return the empty
      body, or document the divergence as deliberate. Test: assert the 404 body is empty (or, if
      kept, pin the wording).
- [x] Resolve the report path namespace/case split. `/portfolio/*` is kebab-case and `/reports/*`
      is snake_case, but `/reports/tax-report` (kebab) sits beside `/reports/rollover_consistency`
      (snake), and route names diverge from module names (`mic_validation` →
      `/reports/exchange_mic_validation`). Pick one scheme for the whole report surface and either
      normalise the paths (UI `REPORTS` config and docs following) or state the rule in
      `docs/API.md`. Test: `doc_checks` asserts the chosen rule is stated (and any path change
      keeps the UI config and route table in agreement).
- [x] Normalise the verb and success status for reads. 12 read reports are `POST`+JSON body
      (`overview`, `activity`, `performance`, `period-performance`, `unrealised-gains`,
      `parcel-optimiser`, `wash_sales`, `row_history`, `tax-report`, the two `what-if`s) while
      sibling reads are `GET`+query (`open-parcels`, `realised-gains`, `net-capital-gain`,
      `tax-summary`, the cross-checks), and `POST /closing_prices/fetch` answers `200` where every
      other "returns the created row" POST answers `201`. Move scalar-parameter reads
      (`listing_id`, `from`/`to`, `window_days`, `tax_year`) onto `GET`+query, keep `POST` only for
      genuinely complex bodies (price maps, allocations), and make "returns the created row"
      uniformly `201`. Breaking: the UI `REPORTS`/`ACTIONS` config and docs follow. Test: router
      tests assert each read's verb and status; the UI bundle still drives them.

Closed 2026-09-25. The DELETE 404s name the key (`CrudEntity::missing_row_body`), `GET /rights_sales/{id}` answers the shared empty-body 404, the report surface has one case rule per namespace (`/reports/tax-report` → `/reports/tax_report`, stated in `docs/API.md` and pinned by `doc_checks::report_path_namespace_case_rule_documented` plus `reports::tests::report_paths_use_their_namespace_case`), and the scalar-parameter reads moved from `POST`+body to `GET`+query (`activity`, `period-performance`, `parcel-optimiser`, `wash_sales`, `row_history`, `tax_report`, the franking what-if) while the price-map and allocation-list reads kept their bodies and `POST /closing_prices/fetch` now answers `201`. The verb split is pinned by `reports::tests::report_routes_use_the_read_verb_their_parameters_call_for`, the status by `closing_price`'s fetch test, and the UI's query building by `web::tests::moved_report_reads_are_driven_as_get_with_a_query` over `util.js`'s `queryString` (unit-tested in `src/web/util.test.js`).

## REST API audit — LLM / machine-client surface (2026-09-24)

The API is also consumed by LLMs and scripts, not just the web UI. These close the gaps the audit
found for a non-browser client; the first is the largest.

- [x] Emit a machine-readable API description (OpenAPI or JSON-Schema). The whole contract is
      currently the ~616 KB prose `docs/API.md` (1,936 lines), with the critical global rules
      (money/quantity as JSON strings, `deny_unknown_fields` on every body) stated only in prose at
      the end (docs/API.md:1838–1865). Generate it from the route table + serde structs and pin it
      with a test (like the existing `doc_checks`) so it cannot drift. Test: a check that the
      generated spec covers every route and carries the string-decimal and deny-unknown-fields
      rules.
- [x] Pin outbound money/quantity serialization. Responses serialize `Decimal` as strings only by
      accident of `rust_decimal`'s default (`Cargo.toml:16` `features=["maths"]`; no
      `serialize_with` anywhere in `src`). A dependency-feature change would silently turn every
      money/quantity field into a float. Add an explicit string codec (the write-side mirror of
      `infra::decimal::strict_decimal`) and a test that a money field serializes as a JSON string.
- [x] Standardise error responses for machine clients. Every error body is `text/plain` with a
      status/body matrix (422/400/413/502/503 carry text; GET 404 is empty; internal 500 is empty;
      job 500 carries text). Either adopt one JSON error envelope, or document the matrix as an
      explicit contract in `docs/API.md`. Test: `doc_checks` pins the chosen contract.
- [x] Document list ordering, the POST-for-read set, and pagination as a first-class contract. API
      list order is ascending id/date (not the UI's newest-first), 12 read reports are POST+body,
      and `/reports/row_history` is the only cursor-paginated endpoint (and is shape-polymorphic:
      array vs `{entries, page_size, next_before_id}`). Add one summary table to `docs/API.md` so a
      client can learn these once. Test: `doc_checks` asserts the table (or the per-endpoint
      statements) exists.

Closed 2026-09-25. The audit's largest item landed first: `GET /openapi.json` serves a generated OpenAPI 3.1 document (`src/api_spec.rs`) built from a route table covering every route and from `utoipa::ToSchema` derives on the real request/response types, pinned by coverage, money-string, deny-unknown-fields and ref/uniqueness tests. Outbound money is pinned by two serialization tests that fail the moment a `Decimal` renders as a number, the error-body matrix is one documented contract pinned in `docs/API.md`, the OpenAPI description and `infra::http`'s own tests, and `docs/API.md`'s `## Reading a list` states list ordering, the four POST-bodied reads and `/reports/row_history`'s two cursor-paged shapes in one place.

## REST API audit — API improvements (2026-09-24)

Improvements that change behaviour or add features, called out by the same audit. These go beyond
fixing drift: they make the API safer and cheaper to build against, for the web UI and machine
clients alike. Each is closed by the tests named.

- [x] Non-clobbering writes (B3). `PUT /collection/:id` silently replaces an existing row with no
      body and no created-vs-replaced signal, and there is no version/`If-Match`, so a stale or
      mistaken write clobbers a record with no confirmation. Make the outcome explicit and/or add
      optimistic concurrency: answer `201`+row on create and `204` (or a body) on update, or add a
      version/ETag so a stale read cannot overwrite a newer row. Test: round-trip tests assert the
      create-vs-update signal; an `If-Match`-stale write answers `412`/`409`.
- [x] Server-side filtering (and paging) on the workhorse lists (B4). Only `closing_prices`,
      `attachments`, and `report_snapshots` accept query filters; `GET /trades`, `/income`,
      `/listings`, `/amma_statements`, … return the whole table, so a client fetches and filters
      entire tables. Add the obvious filters (`?listing_id=`, `?from=`/`?to=`,
      `?holding_account_id=`) and/or cursor paging to the entity lists. Test: API tests assert each
      filter narrows the result and unknown params still `422`.
- [x] Make the as-at default explicit (B8). Omitting the as-of date silently means "today's live
      position" (`as_of_or_today`), never the open-ended sentinel, and `/portfolio/overview` has no
      as-of parameter at all while `performance`/`unrealised-gains`/`parcel-optimiser` each default
      their own. Expose `as_of_date` uniformly on the valuation reports (overview included) and
      state the default in `docs/API.md`. Test: API tests assert the omitted-date default and the
      as-of behaviour; `doc_checks` pins the stated default.
- [x] Filter errored rows out of `GET /closing_prices` (B7). The list returns errored rows
      (`status:"error"`, `price:null`) interleaved with ok ones, so a client computing a valuation
      must filter client-side. Add a `?status=ok|error` filter (or `include_errored=false` default)
      so clean prices are one call. Test: API tests assert the filter.
- [x] Rate-limit / lock out `POST /login` (B6). There is deliberately no lockout or rate limit
      (`infra/auth.rs:43-44`), so a single-credential deployment can be brute-forced. Add a bounded
      per-source lockout (or rate limit) — this reopens the prior scope decision, now that the API
      is also a script/LLM surface. Test: a lockout test over a small attempt budget.

Closed 2026-09-25. All five improvements landed: `PUT /<collection>/{id}` reports its create-vs-replace outcome (`201` with the created row, `204` on a replace, decided inside the write transaction), the workhorse entity lists take server-side filters, every valuation report takes `as_of_date` (omitted = today's live position, stated in `docs/API.md`), `GET /closing_prices` takes `?status=ok|error`, and `POST /login` carries a bounded per-source lockout (5 failures, 5-minute cooldown, `429` + `Retry-After`, keyed on the peer IP). This was the last open section of the audit.

Corrections 2026-09-26, from the per-commit review in `REVIEW.md` — recorded here because both items were ticked with an acceptance criterion that was not met, and the archive is what outlives the review:

- "Pin outbound money/quantity serialization" asked for an explicit write-side codec *and* a test. No codec was added: `rust_decimal`'s `serde-str` gates only the `Deserialize` impls, so the string wire format is still the default that comes of `serde-float` being off. That is a deliberate choice — a `serialize_with` attribute would have to be repeated on hundreds of fields and would drift — but it means the **tests are the whole guard**, which `Cargo.toml` and `infra::decimal` now say plainly instead of claiming a codec.
- "Filter the workhorse entity lists server-side" asked that an unknown parameter "still `422`". It answers `400`: the query decoder rejects it before the handler runs, and `400` is this API's documented status for an unreadable query string. Making it `422` would have singled these routes out from every other query-decoding route, so the criterion was wrong rather than unmet. `every_list_route_refuses_an_unknown_parameter` pins `400` across all 23 of them. (`POST /jobs/{name}` is the one route that does answer `422`, because it reads the rejection itself so a misspelt `?suffix=` cannot take an unlabelled backup.)

## 2026-10-04 HTTP API consistency sweep

A sweep of all 171 routes for consistency and common API pitfalls, probed against a live server.
**The only clients of this API are the web UI and Claude Code**, so none of the items below needs
a compatibility shim, alias or deprecation period: rename/re-shape the route, update `config.js`,
`api_spec.rs`'s `ROUTES`, `docs/API.md` and the tests in the same change.

- [x] **Cross-site writes and DNS rebinding with `[auth]` off.** A plain HTML form on any site could
  `POST` to `http://127.0.0.1:3000` — the bodyless operations (`demerge`, `exchange`, `recognise`,
  `vest`, `POST /jobs/:name`, `regenerate_provisional`), the text-body imports (a forged
  `/rba_fx_rates/import` would plant FX rates the import then never overwrites) and the multipart
  upload all accepted it — and the server answered any `Host`, so a rebinding page could read the
  whole portfolio. Fixed by `infra::request_guard` (`403` for a write the browser marks
  cross-site/same-site or whose `Origin` ≠ `Host`; with no `[auth]`, `403` for a `Host` that is not
  `localhost`, an IP literal or in the new `allowed_hosts` config key). Tests:
  `infra::request_guard::tests::*`, `infra::config::tests::allowed_hosts_are_normalised_and_a_non_name_is_rejected`.
- [x] **Three lists silently ignored their query string.** `GET /rights_sales?listing_id=3` answered
  the whole table; likewise `/exchange_holidays[/:mic]` and `/listings/:id/renames`. `/rights_sales`
  now filters by `listing_id`/`holding_account_id`/`rights_action_id`/`from`/`to`, the holiday lists
  by `from`/`to`, and the renames list refuses any parameter; the `HandWrittenIgnoringQuery` list
  kind is gone. Tests: `entities::tests::every_list_route_refuses_an_unknown_parameter` (now drives
  the path-narrowed lists too), `rights_sale::tests::api_sell_rights_returns_201_then_lists_gets_and_deletes`,
  `exchange_holiday::tests::api_lists_narrow_by_an_inclusive_date_range`.
- [x] **Stale Error-body row.** `docs/API.md` named `GET /report_snapshots/series` "with an unknown
  report slug" as an empty `404`; that route takes no slug — it is `GET /report_snapshots/{report}/{date}`.
  Pinned by the existing `doc_checks::error_body_matrix_pins_every_status_and_shape` table read.
- [x] **One path casing.** Paths mix snake_case (entities, `/reports/*`, `clear_unpriced_before`,
  `regenerate_all`) with kebab-case (`/portfolio/net-capital-gain`, `parcel-optimiser`,
  `period-performance`), and `/reports/franking_at_risk/what-if` mixes both in one path. Pick one
  (snake_case matches the table/column names and the majority) and rename the rest. While there:
  `api_spec::query_parameters` carries a dead `"/reports/net-capital-gain/what-if"` arm that no route
  matches (the route is `POST /portfolio/net-capital-gain/what-if`).
  Done: every `/portfolio/*` report and the `what-if` qualifier are snake_case (`/portfolio/net_capital_gain/what_if`,
  `/reports/franking_at_risk/what_if`, …) and the dead arm is gone. Tests:
  `reports::tests::report_paths_are_snake_case` (every segment of every report route),
  `doc_checks::report_path_namespace_case_rule_documented` (the rule, and the old spellings only in its dated note).
- [x] **Rename routes use two URLs.** A rename is created at `POST /listings/:id/rename` but listed
  at `GET /listings/:id/renames` and undone at `DELETE /listings/:id/renames/:rename_id`. Move the
  create onto the collection (`POST /listings/:id/renames`).
  Done: `POST`/`GET /listings/:id/renames` share one route. Tests: `api_spec`'s
  `every_served_route_is_documented_and_nothing_else_is`, and every `listing_rename` API test now posts there.
- [x] **Writable URLs that cannot be read.** `PUT`/`DELETE /closing_prices/:listing_id/:price_date`
  and `DELETE /listings/:id/renames/:rename_id` answer `405` to a `GET` of the same URL. Add the
  GET-one (empty `404` when absent, per the contract), or record why not. (`/sells/:id` reading
  through `/trades/:id` is deliberate — say so where the Sells section lists its routes.)
  Done: `GET /closing_prices/:listing_id/:price_date` and `GET /listings/:id/renames/:rename_id`
  (the rename only under its own listing), both on the empty-`404` contract; the Sells section
  says why `/sells/:id` has none. Tests: `api_spec`'s `every_writable_url_is_readable` (every
  `PUT`/`DELETE` path has a `GET`, or is classified with where it is read — `/sells/{id}`,
  `/income/{id}/reinvest`), `every_get_one_route_answers_the_empty_404`,
  `closing_price::tests::delete::api_get_one_reads_the_row_the_put_and_delete_address`,
  `listing_rename::tests::api_get_one_reads_the_rename_the_undo_addresses`,
  `doc_checks::sells_section_says_why_there_is_no_get_one`.
- [x] **One status and wording for an unrecognised query parameter.** `POST /jobs/:name` answers
  `422` "cannot read the query string: …" while every other route answers `400` with axum's "Failed
  to deserialize query string: …" (and JSON body rejections keep axum's "Failed to deserialize the
  JSON body into the target type:" prefix). Settle on one status for a query rejection and one
  wording shape, ideally by a shared `Query`/`Json` extractor wrapper rather than per handler.
  Done: `infra::extract`'s `Query`/`Json` replace axum's in every handler — a query rejection is
  always `400`, a body keeps axum's status (`422`/`400`/`415`/`413`), and both read
  `cannot read the query string|request body: <serde reason>`; the job trigger's hand-mapped `422`
  is gone, and `docs/API.md` gains a "Rejected requests" section. Tests:
  `infra::extract::tests::every_rejection_reads_cannot_read_the_part` (real routes, query and body),
  `infra::extract::tests::no_handler_takes_axums_own_query_or_json` (no module imports axum's),
  `doc_checks::rejected_request_shape_documented`, and the two `scheduler` trigger tests now at `400`.
- [x] **Create-like POSTs disagree on `200` vs `201`.** `POST /closing_prices/fetch` and
  `/amma_statements/:id/generate_adjustments` answer `201`; `POST /report_snapshots/generate`,
  `/closing_prices/backfill` and the three `/…/import` feeds answer `200` though they can create rows.
  Decide the rule (e.g. `201` only when the response is the created resource; `200` for a summary of
  a batch that may also update) and state it in `docs/API.md`'s "Creating a record", aligning any
  route that breaks it.
  Done: the rule is `201` only when the call created a new resource and its body is that resource
  (a GET-readable row, or the group of rows one operation created together); a batch summary or a
  replace answers `200` — stated in `docs/API.md`'s "Creating a record" and the Response-codes rows.
  The imports, backfill and snapshot runs already fit; `POST /closing_prices/fetch` now answers
  `200` when it replaces a stored row (same id) and `201` only for a day with none. Tests:
  `api_spec`'s `every_201_post_answers_the_resource_it_created` (every `201` POST's response is a
  GET-served row or a classified created group) and `the_success_statuses_are_derived_from_the_verb_or_the_handler`
  (now reads every status a handler names, so fetch and generate_adjustments carry `[200, 201]`),
  `closing_price::tests::fetch::api_fetch_replaces_errored_row_and_returns_it`,
  `doc_checks::post_status_rule_documented`.

## 2026-10-04 agent usability of the HTTP API

The question "what would make the API easier for agents to use?", answered by reading the served
`GET /openapi.json` from a live server (177 operations, 207 schemas, ~397 KB) against how agents
actually drive it: Claude Code running `curl` against the deployed server. As with the consistency
sweep above, the API has no external clients, so nothing here needs a compatibility shim. Ordered
by value; the first item is a bug.

- [x] **Every `200` response in the OpenAPI document has lost its schema.** `api_spec::operation`
  (`src/api_spec.rs:1879`) attaches the response body only to a `201` —
  `let body = if status == 201 { response } else { Body::None };` — so 109 success responses
  (every list, every GET-one, every report, every import/backfill summary) are published as a bare
  `"OK"` with no `content`. The `ROUTES` rows are right (`GET /trades` records
  `JsonArray("Trade")`) and the handler-type scans pin them; nothing reads the *emitted* document
  back. Introduced by `59bc2e6` (2026-09-25); the intent, per its own comment, was "a `204` never
  has a body". Fix to `status != 204`, and add a test walking the served document: every `ROUTES`
  row's response `Body` appears under each of its non-`204` statuses.
  *Done 2026-10-04:* `operation` now attaches the body to a `200` or `201` — not `status != 204`,
  because the login/logout `303` is a redirect with no body either (its row records `text/html`
  for the `200` page re-render, which is not a listed status). Pinned by
  `api_spec::tests::every_success_response_carries_its_routes_body`, which reads the emitted
  document back and checks each row's `Body` per variant (a `$ref`, an array of `$ref`s, integers,
  a free-form object, or the media type) under every `200`/`201`, and no `content` under a
  `204`/`303`; it fails on `get /listings's 200 has lost its body` with the old line.
- [x] **No `operationId` or `tags` on any operation** (0 of 177). Every OpenAPI-to-tool adapter
  (function calling, MCP bridges) names a tool by its `operationId`. Derive one per route from the
  verb and path (`listTrades`, `getTrade`, `createSell`, `runPortfolioOverview`, …), unique and
  stable, and tag each operation with its `docs/API.md` section. Test: every operation has an
  `operationId`, they are unique, and every operation carries exactly one tag from a fixed list.
  *Done 2026-10-04:* `api_spec::operation_id` derives the id from the verb and path — a verb word
  (`list` for a `GET` answering an array, else `get`; `create` for a `201` `POST`, else `run`;
  `upsert`; `delete`), the literal segments in PascalCase, then `By` + the path parameters
  (`getExchangeHolidaysByMicAndDate`) — and `tag_for` files each route under the `docs/API.md`
  `## ` section of the longest matching prefix in the `TAGS` table (so `/income/{id}/reinvest` is
  `DRP reinvestment`); the document's top-level `tags` lists the used ones in `docs/API.md` order.
  Pinned by `every_operation_has_a_unique_id_and_one_documented_tag` (ids present, plain and
  unique; exactly one tag per operation, each a real `## ` heading of `docs/API.md` and listed
  once at the top level) and `the_tag_list_follows_the_published_routes` (no Authentication tag
  without `[auth]`). Documented in `docs/API.md`'s OpenAPI description section.
- [x] **775 of 1341 schema properties carry no description.** utoipa takes a field's `///` doc
  comment as its description, so this is doc comments on the struct fields — request bodies first,
  since that is where an agent's mistakes are made. Test: every property of every request-body
  schema (the ones `ROUTES` names as a request `Body`) has a non-empty `description`.
  *Done 2026-10-04:* 196 undescribed request-body fields across 29 schemas now carry doc comments
  stating unit/currency, convention, default-when-omitted and validation (each corporate-action
  field names the action types it belongs to), worded from `docs/SCHEMA.md` and the write-time
  checks. Pinned by `api_spec::tests::every_request_body_property_is_described`, which walks the
  served document from each `ROUTES` request body through every schema it `$ref`s (48 schemas, 261
  properties) and names any property without a description — counting the `oneOf` `$ref` branch
  where utoipa puts an `Option<Enum>` field's comment. Response schemas (the remaining ~580
  properties) are not held to the rule; `docs/API.md`'s OpenAPI section says so.
- [x] **The contract is too large to load.** The OpenAPI document is ~397 KB (~100k tokens) and
  `docs/API.md` ~670 KB, so an agent can read neither whole. Add (a) a compact route index — one
  line per route, verb + path + summary, generated from `ROUTES` (a served route or a checked-in
  doc pinned to `ROUTES` by a test), and (b) a `?tag=` filter on `GET /openapi.json` serving just
  that tag's operations plus the schemas they reach (depends on the tags item). Tests: the index
  lists exactly the `ROUTES` rows; a tag slice is a valid document whose every `$ref` resolves and
  that carries no operation from another tag.
  *Done 2026-10-04:* (a) `GET /openapi/index` serves a plain-text index (~27 KB against the
  document's ~448 KB): one line per operation — verb, path, `operationId`, summary — grouped under
  each tag's heading, which names the `?tag=` request for that slice; built by `route_index` from
  `published_rows`, the same list `document_for` now iterates. (b) `GET /openapi.json?tag=` serves
  one tag's operations plus every schema they reach through `$ref`, transitively (`slice_where`).
  Because the `Portfolio reports` tag alone is ~200 KB (half the document), a `?operation=<id>`
  slice was added beside it — the usual agent path is index → `operationId` → that one operation
  (~20 KB for `createSells`). An unknown tag/id, or both at once, is `400` with a reason. Pinned by
  `the_route_index_lists_exactly_the_published_operations` (both `[auth]` shapes),
  `a_tag_slice_is_a_self_contained_document_of_that_tag_alone`,
  `an_operation_slice_is_a_self_contained_document_of_that_operation_alone` (every tag / every
  operation: exactly its operations unchanged, every `$ref` resolves, no unreached schema, same
  envelope) and `the_tag_slice_and_the_route_index_are_served`. Documented in `docs/API.md`'s
  OpenAPI section and `400` row, FEATURES and README.
- [x] **No way to preview a tax-relevant write before it is committed.** Only
  `POST /amma_statements/:id/generate_adjustments` has a preview (`"preview": true`). An agent
  entering figures from a statement wants to see the stored result first. Add `?dry_run=true` to
  the writes whose result is computed rather than echoed — `POST /sells`, the corporate-action
  operations (`participate`, `demerge`, `exchange`, `exercise`, `sell_rights`, `recognise`),
  `POST /transfers`, `POST /ess_statements/:id/vest`, `POST /income/:id/reinvest`: run the full
  write and its write-time validation inside `write_tx`, read the rows back inside it (as creates
  already do), roll back, and answer `200` with the same body the commit would have answered `201`
  with (the `200`/`201` rule in `docs/API.md`'s "Creating a record"). Tests: per route, a dry run
  answers `200` with the body a real run then answers `201` with, writes no row (and no
  `row_history` entry), and refuses a bad body with the same `422`.
  *Done 2026-10-05:* `infra::db::WriteMode` ends each of the ten writes' transactions — `finish`
  commits, or rolls back for a dry run — decoded from `?dry_run=` by `infra::http::DryRunQuery`;
  each handler answers `200` or `201` by it. Six of the ten (vest, reinvest, exercise,
  sell_rights, recognise, participate) read their created rows back on the pool *after* the
  commit, against the CLAUDE.md read-inside-the-transaction rule; they now read inside it, which
  the dry run needs (`rights_sale::db_get` now takes a connection). The rollback also undoes the
  `AUTOINCREMENT` bump, so the preview's ids are the real run's. Pinned per route by
  `*_dry_run_previews_without_storing` in each module, over `ApiClient::assert_dry_run_previews`
  (dry run `200`, the whole database — `row_history` and `sqlite_sequence` included —
  byte-identical afterwards, the real run `201` with the same body) and
  `assert_dry_run_refuses_alike` (the same `422` text both ways, nothing stored); all ten fail
  with `WriteMode::DryRun` committing. The OpenAPI rows carry `[200, 201]` and a `dry_run` query
  parameter. Documented in `docs/API.md`'s new "Previewing a write" section, the `200`/`201`
  rule, and FEATURES. The AMMA generation keeps its body-level `"preview": true`.
- [x] **The live-database recipes live only in one user's agent memory.** The data-entry recipes
  (the annual VDHG AMMA entry, the quarterly ICE E*TRADE statement entry, triggering
  `rba-fx-import` when the tax summary `422`s), the deployed server's address, the bearer-token
  auth and the request rules an agent trips on (money/quantity as JSON strings, unknown fields
  refused, `PUT` replaces) belong in a Claude Code project skill at `.claude/skills/live-api/`
  — **not committed**: it names the private host and describes the real portfolio, so add
  `/.claude/skills/live-api/` to `.gitignore`. Test (the skill itself is outside the repository,
  so the test pins only the exclusion): a `doc_checks` test that `.gitignore` excludes the
  skill's directory.
  *Done 2026-10-05:* `.claude/skills/live-api/SKILL.md` written on the owner's machine — the
  deployed address, bearer auth read from `$SHARE_TRACKER_TOKEN` (never written down), the
  `/openapi/index` → `?operation=` lookup path, the request rules (string decimals, unknown
  fields refused, `PUT` replaces, `?dry_run=true`, backup before bulk changes) and the three
  recipes (missing FX rate, quarterly ICE statement, annual VDHG AMMA). `.gitignore` excludes
  `/.claude/skills/live-api/`, pinned by `doc_checks::the_live_api_skill_is_never_committed`
  (the line is present and no later `!` rule re-includes it).
- [x] **A missing body field is reported one at a time.** serde stops at the first
  (`missing field 'trade_type'`), so an agent filling a large body may take several round trips.
  The `required` lists in the schemas largely answer this once the first item is fixed; decide
  whether that is enough or whether a body rejection should name every missing field. Test: either
  every request-body schema's `required` list matches its struct's non-`Option`, non-defaulted
  fields, or a rejection test names two missing fields at once.
  *Decided 2026-10-05: the `required` lists are enough.* Naming every missing field would mean
  re-deriving serde's requirement outside serde; the schema already states the whole list, and an
  agent reaches it in one `GET /openapi.json?operation=<id>`. What was missing was any proof the
  list is right, so `api_spec::tests::every_request_body_schemas_required_list_is_what_the_decoder_requires`
  now drives every JSON request body through the real router: a body with every property set
  (one item per array, recursively) must decode, and removing any one property must answer
  `missing field` naming it exactly when the schema lists it as required — 15 mismatches reported
  when `ReinvestBody`'s and `SellAllocationInput`'s lists were deliberately corrupted; none
  today. Documented in `docs/API.md`'s OpenAPI section.
- [x] **A GET-one for a missing row is an empty `404`**, so an agent cannot tell a mistyped path
  from an absent row (an unmatched path is an empty `404` too). Decide whether the GET-one `404`
  should carry a plain-text reason (`no trade with that id`, the DELETE wording) — this reverses
  the documented empty-`404` contract (`every_get_one_route_answers_the_empty_404`, the Error-body
  matrix), so it is the owner's call. Test: whichever contract is chosen, pinned by that test and
  `doc_checks::error_body_matrix_pins_every_status_and_shape`.
  *Done 2026-10-08 (owner's call: carry the reason):* `ApiError`'s empty `NotFound` variant is
  gone — its one `404` variant always carries a body — so the only empty `404` left is axum's for a
  path no route matches, and the body alone tells a mistyped URL from an absent row.
  `get_handler` answers `CrudEntity::missing_row_body`, the wording its DELETE already used, whose
  default now names the key column (`no currency with that code`, `no exchange with that mic` —
  the exchange override became redundant); the seven hand-written GETs name their own key
  (`no rename of that listing with that id`, `no stored price for that listing and date`,
  `no snapshotted report named "nope"`, …). Pinned by
  `api_spec::tests::every_get_one_route_names_the_missing_row_in_its_404` (every GET-one in the
  route table plus the attachment-content download: `404`, `text/plain`, a `no …` body; an
  unrouted path still empty) and `doc_checks::error_body_matrix_pins_every_status_and_shape`, whose
  empty `404` row is now axum-produced like the `405`. Documented in `docs/API.md`'s Response
  codes and Error-body contract, the OpenAPI description and per-route `404` response, and
  CLAUDE.md's API conventions.

Considered and not proposed: an MCP server exposing the API as tools. 177 tools is too many for an
agent to choose among; the items above (a complete spec with `operationId`s, a compact index, tag
slices, and the project skill) give most of the benefit, and a small curated MCP tool set can be
revisited once they land.
