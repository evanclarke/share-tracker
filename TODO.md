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

The 2026-10-04 HTTP API consistency sweep — cross-site write and DNS-rebinding guard, strict lists,
one path casing, readable writable URLs, one rejection shape, and the `200`/`201` rule — is closed
and archived in [`DONE/api.md`](DONE/api.md).

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
- [ ] **A GET-one for a missing row is an empty `404`**, so an agent cannot tell a mistyped path
  from an absent row (an unmatched path is an empty `404` too). Decide whether the GET-one `404`
  should carry a plain-text reason (`no trade with that id`, the DELETE wording) — this reverses
  the documented empty-`404` contract (`every_get_one_route_answers_the_empty_404`, the Error-body
  matrix), so it is the owner's call. Test: whichever contract is chosen, pinned by that test and
  `doc_checks::error_body_matrix_pins_every_status_and_shape`.

Considered and not proposed: an MCP server exposing the API as tools. 177 tools is too many for an
agent to choose among; the items above (a complete spec with `operationId`s, a compact index, tag
slices, and the project skill) give most of the benefit, and a small curated MCP tool set can be
revisited once they land.
