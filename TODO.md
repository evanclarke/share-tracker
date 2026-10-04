# TODO

Items are only marked done when a passing test exists for them.

This file holds only open / in-flight work. Completed and decided (out-of-scope / not-reproducible)
sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md). When a
section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see
CLAUDE.md.

A section records one finding, and its heading names where it came from — a REQUIREMENTS entry, a
[SCENARIOS.md](SCENARIOS.md) section, or a dated review pass.

The last full pass recorded here — the 2026-09-17 code review — is closed and archived in
[`DONE/reviews.md`](DONE/reviews.md) (26 sections), with its summary and closing narrative in
[`DONE/verification-passes.md`](DONE/verification-passes.md). Everything before it is likewise
archived; the maintained record of what has been verified is SCENARIOS.md's
[Verification status](SCENARIOS.md#verification-status) table, and of what was built and decided,
the [DONE.md](DONE.md) index. The 2026-09-24 REST API audit — its documentation fixes, consistency fixes,
machine-client surface and API improvements — is closed and archived in
[`DONE/api.md`](DONE/api.md), which is the record of what it found and what
answered each finding.

The nine items the 2026-09-26 per-commit review deliberately left — the structural
enforcement it judged out of proportion, and the three decisions it would not make on the
owner's behalf — are closed too, and archived with the review itself in
[`DONE/reviews.md`](DONE/reviews.md).

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
- [ ] **One status and wording for an unrecognised query parameter.** `POST /jobs/:name` answers
  `422` "cannot read the query string: …" while every other route answers `400` with axum's "Failed
  to deserialize query string: …" (and JSON body rejections keep axum's "Failed to deserialize the
  JSON body into the target type:" prefix). Settle on one status for a query rejection and one
  wording shape, ideally by a shared `Query`/`Json` extractor wrapper rather than per handler.
- [ ] **Create-like POSTs disagree on `200` vs `201`.** `POST /closing_prices/fetch` and
  `/amma_statements/:id/generate_adjustments` answer `201`; `POST /report_snapshots/generate`,
  `/closing_prices/backfill` and the three `/…/import` feeds answer `200` though they can create rows.
  Decide the rule (e.g. `201` only when the response is the created resource; `200` for a summary of
  a batch that may also update) and state it in `docs/API.md`'s "Creating a record", aligning any
  route that breaks it.
