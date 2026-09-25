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
