# Commit review: ac3a361..HEAD

Reviewed one commit at a time, each by a fresh subagent, against `CLAUDE.md` conventions.

## Summary

Twenty commits, one subagent each, verified against the tree as of each commit. The range is the
2026-09-24 REST API audit: mostly documentation, plus five substantive behaviour changes
(`ec975e6`, `ea21d55`, `a8da1ad`, `59bc2e6`, `9400e70`, `8da9424`, `a80370b`, `a9da849`). Quality is
high — the risky mechanics were got right almost everywhere (no SQL injection in the new filters,
`deny_unknown_fields` and exact-decimal money parsing survive the body→query move, create-vs-replace
is decided inside `BEGIN IMMEDIATE`, the login lockout gates before the Argon2 verify). What the
range accumulates instead is **contract drift that the tests cannot see**.

### Act on these first

1. **`59bc2e6` — the PUT clobber fix shipped server-side only.** On a natural-key **Create** form the
   UI PUTs a user-typed key and toasts "Saved." on the new `204 Replaced`, so re-adding exchange
   `XASX` silently overwrites the seeded row's settlement days — changing T+n on every ASX trade.
   The one case the commit exists to prevent.
2. **`8da9424` — back-dated valuation uses today's price and FX.** The new "As-of date" control on
   Overview combines with `app.js`'s hardcoded `live: true`; the docs assert the opposite with no
   carve-out (`docs/API.md:1395` is outright false under `live`).
3. **`9400e70` — owner filters silently drop NULL-owner rows.** `investment_expenses` and
   `interest_income` have deliberately-nullable owner columns, so iterating
   `?holding_account_id=N` over accounts understates deductions with no error. Known to the author;
   documented only in a test comment.
4. **`9053adb` — two tests in `doc_checks.rs` now enforce mutually inconsistent contracts.** The
   matrix says `415` carries a body; `RETURNED_WITH_BODY` omits it and `assert_eq!`s the list, so
   the doc **cannot be fixed without editing a test**. Still open at HEAD.
5. **`a9da849` — the lockout budget is check-then-act** (effective budget `5 + concurrency`), and
   `Source::Peer` keys full IPv6 addresses, so a /64 defeats it for free.

### Cross-cutting themes

- **Hand-transcribed mirror lists** — the dominant theme, and the one CLAUDE.md already forbids for
  `config.js` option lists. Instances: `RETURNED_WITH_BODY`, the error-matrix `EXPECTED`, the
  `cases` table, api_spec's `DESCRIPTION` copy, the 201-create list, the ordering table pin, the
  `FILTERED` tables (×2), `PUT_ROUTES`' second copy in api_spec. `a9da849` had to hand-edit four of
  them at once for 429 — it got all four right, but nothing would have failed had it missed one.
  Several of these lists are *documented as derived* when they are typed in by hand.
- **Doc-against-doc tests.** Four tests assert substrings of `DESCRIPTION` against `DESCRIPTION`;
  several assert that `docs/API.md` contains strings `docs/API.md` was just edited to contain. They
  fail only when someone edits one copy and forgets the other; changing the *behaviour* trips none
  of them.
- **The OpenAPI document is the weakest surface.** It declares **no query parameters at all** —
  while `ea21d55`, `9400e70`, `8da9424` and `a80370b` moved or added ~20 routes' worth onto query
  strings — has no `servers` (so a `base_path` deployment publishes 404-ing paths), no
  `securitySchemes` (despite bearer auth being the feature's stated audience), and success-only
  responses. Each later commit extended the gap and pinned its own addition as prose.
- **Ticked TODO items whose stated acceptance criteria were not met**, with no note recording the
  deviation: `ec975e6` (`/report_snapshots` still mixes cases), `59bc2e6` (no If-Match test),
  `9400e70` (422 vs the 400 actually shipped), `8da9424` (`parcel-optimiser` untouched),
  `262e4e8` (no write-side codec was added). Each reads as fully done once archived.
- **One stale sentence, three commits.** `docs/API.md`'s "the two `5xx`s that are not internal
  faults" omits `502`; `962ffaa` introduced the pin, `9053adb` relocated the sentence directly under
  a table refuting it, `a9da849` rewrote its ending — none fixed the count.
- **`CLAUDE.md` itself is now stale in three places**: the `PUT → 204` rule (false since `59bc2e6`),
  `POST /reports/row_history` (405 since `ea21d55`), and the Project-structure list (missing
  `src/api_spec.rs`, and `infra/auth.rs` unchanged for the lockout). It is the file the next author
  writes against.
- **Reuse:** the `API_MD.split("## …")` section idiom is inline ~12 times; the
  `listing_id`/`holding_account_id`/`from`/`to` filter struct + impl is duplicated across ~10
  entities (~200 lines), which would also give the bind-not-interpolate rule a single choke point.

### Clean or near-clean

`cfacc75`, `57a0574`, `28eac50`, `dde16ed`, `72046a0`, `1006989`. `a80370b` is notable for being
the first filter added after the NULL-owner finding and correctly not repeating it.

---

## cfacc75 — Record the REST API audit as open work in TODO.md

**Clean.** TODO.md only. Every factual claim the new audit items make was verified against
the tree as of that commit and holds. The removed 24 lines were introductory narrative, not
`[ ]` items or a `##` section, and the one decision it carried (the `0045_autoincrement_audited_ids.sql`
N/A rationale) already survives verbatim at `DONE/verification-passes.md:372`, so the
never-delete rule is not breached.

Nits (not worth fixing alone):
- The exchange item cites `src/entities/exchange.rs:123-124` for "key is `mic`"; at that commit
  `KEY_COLUMN` is line 122 and 123-124 are `ORDER_BY`/`NOUN`. Citation one line low.
- The machine-client item says "12 read reports are POST+body"; the actual count is 11
  (excluding the writing `/report_snapshots/generate`). A doc table built from it would
  transcribe the wrong count.

---

## 41e808a — Add API-improvement items to the audit TODO

**TODO.md only, no code.** Most factual claims verified correct against the tree at that commit.
Several findings, though most describe a TODO state that later commits in this range resolved:

- **TODO.md:73 — wrong count.** "12 read reports are `POST`+JSON body"; the parenthesis itself
  enumerates 11 and the router has exactly 11 POST reads. *Superseded:* 0dfddd5's message
  explicitly calls this "the audit's stale count of twelve POST reads" and documented 11.
- **TODO.md:131 — acceptance criterion contradicts the API's own contract.** It requires unknown
  list params to `422`, but an axum `Query` rejection is `400`, which is this API's documented
  status for an unreadable query string. *Superseded:* the implementing commit 9400e70 used 400
  and pins `every_list_route_refuses_an_unknown_parameter` at `BAD_REQUEST`. Had it been followed
  literally, a bespoke 422 remap would have diverged from every other query-decoding route.
- **TODO.md:120-143 — dropped audit findings.** Items are tagged `(B3)`,`(B4)`,`(B6)`,`(B7)`,`(B8)`;
  B1, B2 and B5 appear nowhere in the repo, and the source audit is not committed. Three findings
  are silently absent — the exact outcome the never-delete rule exists to prevent — and no reader
  can resolve what `B3` refers to or check the section for completeness before archiving it.
- **TODO.md:124-125 — two items that break each other's test.** B3 requires a stale `If-Match`
  write to answer `412`/`409`, while the doc-fix item at TODO.md:41-44 requires dropping `409` from
  docs/API.md's Error-bodies list *and pinning that with a `doc_checks` assertion*. Whichever lands
  second fails the other.
- **TODO.md:18 — stale preamble.** Still says "the three sections below" after this commit makes it
  four. *Superseded* by ea21d55.
- Lower: B3/B4/B6/B7 all change status codes or request shapes, but only B8 names the `docs/API.md`
  update CLAUDE.md requires in the same task — easy to close one green-but-stale.
- Note: applying "uniformly `201`" to `POST /closing_prices/fetch` is loose, since that handler
  replaces an existing stored row as often as it creates one.

---

## 1006989 — Document date_received as a required AMMA statement field

**Substantially clean.** Docs-only. The claim is accurate and complete: `listing_id`,
`tax_year_end_date`, `date_received` are exactly the three `AmmaStatementBody` fields without
`#[serde(default)]` (`src/entities/amma.rs:104-148`). The `doc_checks` pin is correctly placed,
its section split is unambiguous (one `## AMMA statements` heading), and `config.js:263-266`
already agrees.

- **`src/entities/amma.rs:27` / `docs/SCHEMA.md:200` — unannotated informational-only field.**
  `date_received` is read by no calculation or report (only the model, `COLUMNS`, the upsert binds
  and fixtures), yet carries neither the CLAUDE.md-mandated informational-only comment nor the
  SCHEMA.md annotation its siblings have (`capital_losses_applied`, `tax_deferred_amount`,
  `tax_free_amount` all say "Informational only — …"). This commit *is* the task of explaining the
  field: a reader told only that it is required cannot tell that an approximate date changes
  nothing, whereas every other mandatory AMMA field drives a calculation.
- Reuse: `src/doc_checks.rs:915-921` — the six-line "extract a `##` section from `API_MD`" idiom is
  now duplicated eight times (916, 2407, 2507, 2541, 2590, 2654, 3960, 4006). A single
  `md_section(doc, heading)` helper collapses all eight; the file's own "hand-maintained lists at
  instance eight" convention argues for doing it now.

---

## 4f9ebf4 — Complete the 201 Created enumeration in the Response codes table

Factual core correct: the router has exactly the 14 collection `POST` creates the row lists, all
answering 201, and `POST /ess_statements/:id/vest` is 201.

- **`docs/API.md:1802` — "each answering the created row" is wrong for `/transfers`.**
  `POST /transfers` (`src/entities/transfer.rs:817-823`) answers a `TransferGroup`
  `{ transfer, sell, transfer_ins, fee_sale }`, not the created row. A machine client reading this
  row expects a `Transfer` with an id at the top level, as every other entry in the list gives it,
  and mis-parses the response. The row already contradicts itself: `/transfers` appears later with
  the shape stated correctly. Fix: drop it from the id-keyed list or qualify the clause.
- **`src/doc_checks.rs:533` — the ESS-vest assertion is vacuous.** `assert!(row.contains("vest"))`
  is satisfied by the substring inside `/in`**`vest`**`ment_expenses`, which this same commit added
  to the row. Delete the ESS-vest clause and the test stays green while the doc regresses to exactly
  the omission this commit was written to fix. Assert on `"/ess_statements/:id/vest"`.
- **`src/doc_checks.rs:539-553` — 5 of the 14 collection assertions are also vacuous.** The loop
  does `row.contains(collection)` over the whole row rather than the parenthetical, and
  `/listings`, `/income`, `/corporate_actions`, `/amma_statements`, `/ess_statements` each already
  occur in the row's *operation* clauses (`/listings/:id/rename`, `/income/:id/reinvest`, …). The
  pin's own doc comment claims it "must name every id-keyed collection create"; drop
  `/corporate_actions` and `/income` from the list and it still passes. Fix: slice the cell between
  `"the id-keyed collections"` and `"see [Creating a record]"` and assert against that substring.
- Reuse: the 14-name list is hand-maintained. `src/web.rs`'s
  `every_creatable_entity_has_a_route_for_the_create_its_form_uses` already derives the creatable
  set from the live router by probing for 405. Deriving this pin the same way would make it
  self-maintaining and would additionally catch a *new* 201-create collection the row omits —
  which the current test cannot.

---

## 57a0574 — Document the preference listing field

**Clean.** Every claim in the new prose verified against `src/reports/franking.rs` (90 vs 45 days
at :147-152, the exclusive day count at :256, the window end at :194), and "nothing else reads the
flag" holds — every other `preference` hit is storage/serde, the audit trigger column list,
fixtures or UI config. Anchors resolve; SCHEMA.md already documented the column.

The new pin (`src/doc_checks.rs:558-592`) is correctly scoped: it slices the `## Listings` section
before asserting, and says why — the bare word "preference" already appears in the Tax summary and
Franking at-risk sections, so a whole-file `contains` would pass on unrelated prose. Contrast
4f9ebf4's pin above.

Minor:
- `docs/API.md:142` — the ATO mirror says "90 days for **certain** preference shares"; the 90-day
  rule attaches to preference shares meeting the statutory definition. The new prose reads as
  though the flag *is* the rule. A half-clause ("…that meets the ATO's definition") closes the gap;
  the flag is user-set and unvalidated either way.
- Reuse: eighth inline copy of the `## ` section-slicing idiom (see 1006989 above).

---

## 962ffaa — Drop the stale 409 from the Error bodies list

Factual core correct: `StatusCode::CONFLICT` appears nowhere in `src`, `ApiError` has no 409
variant, and the three new parsers (`error_bodies_paragraph`, `error_bodies_listed_codes`,
`response_code_table_codes`) were hand-traced against the real API.md text and yield exactly
`["400","401","404","413","422","502","503"]` — non-vacuous.

- **`docs/API.md:1840` — the paragraph is still self-inconsistent, in the same way the commit set
  out to fix.** The new list says `502` carries a body, but the paragraph's closing sentence says
  "the two `5xx`s that are *not* internal faults do carry one: a failed job … and a `503`" —
  omitting `502` (`ApiError::BadGateway` → `(StatusCode::BAD_GATEWAY, body)`, `infra/http.rs:180`).
  There are three such shapes, not two. A machine client reads the closing sentence, concludes a
  `502` from the RBA FX import has no body, and discards the "could not reach its source" text it
  should surface. Worse: the new test *pins* that sentence (`doc_checks.rs:713`), so the wrong
  count is now frozen by a test whose whole purpose is preventing this drift.
- **`src/doc_checks.rs:701` — `RETURNED_WITH_BODY` is hand-transcribed but described as derived.**
  The doc comment says it is "derived above from `src/infra/http.rs`" and the test is billed as
  pinning against `ApiError`'s status map; in fact the seven codes are typed in by hand and the
  only mechanical link is one `CONFLICT` substring check. *Already realised in this tree:*
  `a9da849` added `ApiError::TooManyRequests` (429 with a body) and the const had to be edited by
  hand. Update the code and not the const and the test stays green while the doc omits a
  body-carrying status — exactly what the 409 was. The arms are parseable out of `HTTP_RS`
  (`ApiError::X(body) => (StatusCode::Y, body)`); CLAUDE.md already forbids this shape for
  `config.js` option lists.
- **`src/doc_checks.rs:721` — `!HTTP_RS.contains("CONFLICT")` scans the whole file, not the status
  map, and false-positives on SQL.** Every entity upsert writes `ON CONFLICT(id) DO UPDATE`, and
  `infra/http.rs` already has raw SQL in its own test module (:725, :731). Add a generic
  `crud_upsert` helper — the natural next step beside `crud_list`/`crud_get`/`crud_delete` — and a
  *documentation* test fails claiming `ApiError` can answer 409, which is false. Match
  `StatusCode::CONFLICT`.
- **`src/doc_checks.rs:727-734` — the cross-check is one-directional.** Every listed code must have
  a table row, but a body-carrying row need not appear in the list, and nothing stops a `409` row
  reappearing in the table. The mirror image of the bug this commit fixed would pass.

---

## 28eac50 — Document the three DRP residual columns, archive the doc-fixes section

**Essentially clean.** The archiving is verbatim: diffing the removed `TODO.md:21-52` against
`DONE/api.md:3-34` shows exactly one difference, the completed item's `- [ ]` → `- [x]`. Links
resolve from the new location; both new anchors (`#drp-enrolments`, `#drp-reinvestment`) match real
headings. The new pin (`doc_checks.rs:606-638`) is non-vacuous — none of the three column names
appeared in the Trades section before the doc change — and correctly section-scoped.

Every claim in the new prose verified, including the load-bearing one: the residual chain is built
from `trade_type = 'DRP'` rows only (`drp_enrolment::PERIOD_TRADES_FROM_WHERE`), so residuals
written onto a plain `Buy` really are unreachable.

Minor:
- **`DONE.md:18` — the "Covers" cell over-claims.** It says `DONE/api.md` covers the audit's
  "consistency fixes, machine-client contract, and API-improvement items", but all three sections
  are still open in `TODO.md` (:21, :54, :82); the file holds only the documentation-fixes section.
  A reader following the index for the consistency fixes will not find them there.
- **`docs/API.md:415` — "They are informational on a read."** True of client writes, but the columns
  are not inert: `reports::health`'s `DrpChain::closes_across` compares
  `before.residual_carried_forward == after.residual_brought_forward` to suppress a
  missing-distribution alert. "Read-only to clients" is more accurate and does not invite a machine
  client to ignore them. (Note this cuts against the CLAUDE.md informational-only convention —
  the word means something specific in this repo.)
- Reuse: the section-slicing idiom is now inline ~10 times in `doc_checks.rs`.

---

## dde16ed — Name the natural key in the keyed entities' DELETE 404

**Clean.** The behaviour change is correct and complete. `deleted()` now delegates to
`deleted_with_body()` with byte-identical output for the ~10 hand-written deletes; the FK-violation
arm of `delete_handler` is untouched. Coverage is complete: the only `CrudEntity` impls with
`KEY_COLUMN != "id"` are `exchange`, `tax_year_settings` (both overridden) and `currencies` /
`mic_registry`, which expose no DELETE route. The test is a genuine tightening — `text()` returns
`&str`, so it is a whole-body `assert_eq!`, strictly stronger than the previous `contains`.

Low-severity:
- **`src/entities/mod.rs:91-105` — `DELETE_ROUTES`' comment now claims it pins "every entity DELETE
  route", but `DELETE /listings/{id}/renames/{rename_id}` (`listing_rename.rs:286-289`) is absent.**
  Pre-existing omission, but this commit strengthens the claim without closing the gap: change that
  undo 404 to a bare `StatusCode::NOT_FOUND` and the suite stays green while the UI shows "HTTP 404".
- **Nothing structurally forces a natural-key entity to override `missing_row_body`.** Add a
  `DELETE /currencies/{code}` or `/mic_registry/{mic}` later — both already `CrudEntity` with a
  non-`id` key — and it silently answers "…with that id", recreating exactly this bug; the pin only
  catches it if the author remembers the row. A test asserting "every `CrudEntity` with
  `KEY_COLUMN != "id"` overrides `missing_row_body`" makes it structural, in the spirit of the
  project's other classified-list tests.
- `src/infra/http.rs:288` — the doc says the key is passed "so an override can name the value it
  looked for", but neither override uses it and `Key` carries no `Display` bound, so the rationale
  is aspirational. As written the hook could be an associated const, dropping the unused `&String`
  arg in `exchange.rs:128`.
- `docs/API.md:1812` could add the two newly distinguished bodies to its 404 examples. Optional.

---

## 72046a0 — Answer GET /rights_sales/{id} with the empty GET-one 404

**Clean.** rights_sale really was the sole outlier: 21 GET-ones go through the generic
`get_handler::<E>` (which ends `.ok_or(ApiError::NotFound)`) and the other three hand-written ones
already used the bare `NotFound`. The hand-written `get_one` is correctly *kept* — `db_get`
attaches child `rights_sale_allocations`, so collapsing it onto `get_handler` would silently drop
them. The docs were already the contract (the `404` row's "a `GET` … answers with an empty body"
sentence is pinned by `delete_404_reason_documented`), so this moved code *to* the documented
behaviour and owed no doc change. Nothing depended on the removed body; the UI marks
`rights_sales` `deleteOnly`, so it never issues a GET-one.

Minor:
- **`src/entities/mod.rs:105-142` — the drift is only half-pinned.** The DELETE side has the shared
  `DELETE_ROUTES` table whose comment says it "keeps a new entity from drifting again"; the GET side
  has no equivalent, so the exact divergence this commit fixed can recur in another hand-written
  GET-one. Only 4 exist (the other 21 are structurally safe), so a `GET_ONE_ROUTES` sibling would
  close it cheaply — beyond this item's scope.
- `src/entities/rights_sale.rs:1387` — the module now builds `ApiClient::over(…)` inline in 6
  places; CLAUDE.md's test conventions ask for a one-line local `fn client(pool)` at three.
  Pre-existing at 5; this adds the sixth.
- `rights_sale.rs:1389-1390` — axum's default fallback also answers 404-with-empty-body, so this
  assertion alone cannot distinguish "row missing" from "route deleted". Covered in practice by the
  round-trip test at :1371; only matters if that test moves.

---

## ec975e6 — Give the report surface one case rule per namespace

**Mechanically sound — the rename is complete.** Every caller verified: `config.js:621`,
`taxreport.js:483,518`, all four `docs/API.md` occurrences, `web.rs:2823-2824` and ~30 Rust test
sites. `scripts/`, `pkg/`, `schedule.cron`, `.github/`, `migrations/` hold no reference. The UI slug
/ `custom` key / `#/r/tax-report` hash route are correctly left alone. The new
`report_routes_have_a_ui_caller` was hand-run over all 16 `/reports/*` routes — each has a real
caller, so it is not vacuous.

- **TODO.md:36 — ticked `[x]` but the requirement is only partly met.** The item says "Pick one
  scheme for the whole report surface", yet `/report_snapshots/*` still mixes both cases *within one
  namespace* — `holding-series` beside `series`, `regenerate_all`, `regenerate_provisional`,
  `regenerate_range` — and `src/reports/mod.rs:222` explicitly `continue`s past it, with
  `docs/API.md:1182` excusing it as "a namespace of its own". That is the same "excuse rather than
  rename" move the commit message says it refuses elsewhere. CLAUDE.md's "implement a requirement
  fully" says this stays unchecked with a note, or `/report_snapshots` gets its own pinned rule.
  Otherwise the next snapshot endpoint lands in whichever case the author copied and nothing fails —
  the drift this commit set out to stop.
- **`src/reports/mod.rs:268-271` — the new test hardcodes three JS modules via `include_str!`
  instead of the served bundle.** CLAUDE.md is explicit that UI items are asserted against
  `app_js_body` in `web.rs`, the concatenation of every served module. This pins
  `config.js`/`taxreport.js`/`app.js` by name, duplicating existing machinery and bypassing the
  `JS_MODULES` allowlist. A future `JS_MODULES` split — the documented growth path for this UI —
  leaves code and bundle both correct while this test fails, fixable only by hand-editing a second
  module list.
- **`src/reports/mod.rs:162-188` — the route walk scans only flat `src/reports/*.rs`.**
  `read_dir` is non-recursive and the directory is hardcoded, guarded only by a weak
  `assert!(!paths.is_empty())`. Split `tax_report.rs` (already ~5,600 lines) into
  `src/reports/tax_report/http.rs` the way `trade.rs` and `closing_price.rs` were — the precedent
  CLAUDE.md sets — and *both* new pins silently stop seeing those routes while staying green.
- **`src/reports/mod.rs:192-197` — `segment_matches` rejects axum path parameters.** `{`/`}` are
  neither lowercase, digit, nor separator, so the first `/reports/foo/{id}` route fails with
  "segment `{id}` … must be snake_case". `/report_snapshots/{report}/{date}` escapes only via the
  namespace skip. A conventional parameterised report route can't be added without editing the test,
  and the message misdirects to a case problem.
- **`docs/API.md:1527-1531` — a hard-breaking rename with no alias and no dated change note.** The
  `### Annual tax report` subsection already carries two `*Changed 2026-09-17 …*` /
  `*Changed 2026-08-25 …*` notes for display-only changes, so the convention exists and was skipped
  for the one change that actually 404s existing callers. No README Known-limitations entry either.
- `src/doc_checks.rs:97-117` — the docs pin is presence-only; nothing fails if `/reports/tax-report`
  is reintroduced in prose beside the new spelling. The adjacent
  `tax_report_year_picker_scope_documented` does carry the negative assertion.
- Low: `REQUIREMENTS.md:1361-1362` still specifies the old paths — defensible given that file's
  historical framing, but it is the one live document naming an endpoint that no longer exists.
- Nit: `mod.rs:276` uses substring containment, so `/reports/tax_report` is satisfied by a mention
  of `/reports/tax_report/years`, and a path inside a JS comment counts as a caller.

---

## ea21d55 — Move scalar-parameter reads onto GET+query, fetch to 201

**Substantially clean.** No correctness, data-integrity or financial-correctness defect, and no
missed caller. Notably verified:

- Strictness survives the body→query move. All seven query structs keep `deny_unknown_fields`, and
  `serde_urlencoded`'s `Part::deserialize_any` → `visit_borrowed_str` means
  `strict_decimal`/`strict_optional_decimal` parse money as exact decimal strings under `Query`
  exactly as under `Json`. Duplicate keys → 400.
- The 400/422 split is deliberate: every *semantic* refusal stays a handler-owned 422 (non-positive
  `units`, `window_days < 1`, `tax_year` out of range, non-audited `table`, `from >= to`, …); only
  decode failures became 400, and that is documented in four places.
- GET stays safe — `closing_price/live.rs` has no INSERT/UPDATE/`write_tx`, so the two reads that
  can trigger a live quote write nothing. `/report_snapshots/regenerate_range` is genuinely a read.
- The 201 is applied narrowly: `POST /closing_prices/backfill` correctly kept 200 (it answers a
  summary, not a created row).
- Blank-field handling is correct end to end (`forms.js` → `runReport` → `queryString` each drop
  nulls), so an omitted optional never travels as `?window_days=`.

Findings (all low):
- **`CLAUDE.md:27` and `REQUIREMENTS.md:1826` still name the old verbs.** CLAUDE.md says row history
  is "inspected via `POST /reports/row_history`"; the commit updated the *identical* sentence in
  `docs/SCHEMA.md:419` and two Rust doc comments, so these are oversights. CLAUDE.md is normative
  for future work — the next agent reading it writes a `POST` against a route that now answers 405.
- **`src/web/config.js:621` — the new `method: 'GET'` on `tax-report` is dead config that a new test
  pins as if it were live.** `app.js:3190` dispatches `custom: 'tax-report'` to `viewTaxReport`
  before `viewReport` is reached, so `report.method` is never read (the sibling custom entry,
  `snapshots`, carries no `method`); the real verb is hardcoded in `taxreport.js:518`. Someone
  "fixing" the verb by editing config.js alone gets a green suite and unchanged behaviour.
- **The new 400-vs-422 boundary is pinned for only 3 of the 7 moved reads.** `/portfolio/activity`,
  `/portfolio/parcel-optimiser`, `/portfolio/period-performance` and `/reports/row_history` have no
  test that a misspelt or unparseable query param is refused rather than defaulted — and for
  `activity`/`parcel-optimiser` a silently-defaulted `price` is a wrong *money* figure, the class
  `deny_unknown_fields` exists for. Mitigated by `every_request_body_denies_unknown_fields`, which
  scans `Query<` extractors too.
- `docs/API.md` (Listing activity) — a sentence lost its subject in the rewrite: "`price` absent, it
  is live-fetched…".
- `src/reports/mod.rs` — `report_routes()`'s verb scanner takes the first `get(`/`post(` after the
  path literal, searching the rest of the file rather than the route call; it would mis-read
  `.route("/x", post(a).get(b))` and panics for `delete(`/`put(`/a fully-qualified path.
- Nit: `src/reports/tax_report.rs:4355,4481,4530,4584` are the only four non-inlined
  `format!("…{}", ident)` args in `src`.

Judgement call worth surfacing: the old `POST` routes are **removed**, so an existing script or LLM
client gets a bare 405, with no CHANGELOG and no breaking-change note in README/docs/API.md. Fine
for a solo project — but a8da1ad later in this range publishes an OpenAPI document aimed at machine
clients, which makes a one-line note cheap insurance.

---

## a8da1ad — Emit a generated OpenAPI 3.1 description at GET /openapi.json

Solid work: wiring, auth placement (merged before `require_auth` and before `nest`, so the document
is gated and moves under `base_path`), schema derives, `#[schema(as = …)]` collision handling, and
the both-ways route-coverage scan are all correct. `utoipa 6` with `chrono`+`decimal` does give
`Decimal → string` and `deny_unknown_fields → additionalProperties: false` — and the one trap there
(the derive skips `additionalProperties` when a struct emits as `allOf` via `#[serde(flatten)]`)
does not bite, since all seven flatten sites are response types. The `/static/*.js` handling reads
from `web::JS_MODULES` rather than transcribing it — the opposite of the mirror list CLAUDE.md
forbids. The defects are about what the document **omits** and how much of it is really generated.

- **`src/api_spec.rs:1706-1755` — the document declares no query parameters at all.**
  `path_parameters` reads only `{name}` segments; nothing emits `ParameterIn::Query`. After ea21d55
  moved the scalar reads onto query strings, `GET /reports/tax_report` (`?tax_year=`),
  `/portfolio/activity` (`?listing_id=`), `/report_snapshots/series` (`?report=`) and the
  `from`/`to`/`window_days`/`limit`/`before_id`/`disposition` params survive only as English in the
  `summary`. `TaxReportRequest` does not even derive `ToSchema`. And `DESCRIPTION`
  (api_spec.rs:63-65) asserts "Query parameters follow the same rule on the routes that take them" —
  a rule the document carries nowhere. A generated client calls `/portfolio/activity` bare and gets
  a 422 the contract never mentioned; no test fails when a new query parameter is added.
- **`src/api_spec.rs:1536-1547` / `1874-1876` — no `servers` entry, so a `base_path` deployment
  publishes paths that 404.** `api_spec::router()` takes no `base_path` (unlike
  `web::router(base_path, …)` at app.rs:41), so the document can't say where it is mounted; the only
  statement is prose in `info.description`. The FreeBSD deployment behind `--base-path
  /share_tracker` serves the document at `/share_tracker/openapi.json` listing `"/listings"`, which
  tooling resolves against the origin root. `servers: [{url: "/share_tracker"}]` is the one-line fix.
- **No `components.securitySchemes` and no `security`.** docs/API.md documents
  `Authorization: Bearer <api_token>` specifically for non-browser clients — the stated audience of
  this feature — yet the contract is silent on it. Relatedly `/login` and `/logout`
  (api_spec.rs:1166-1189) are listed unconditionally although `infra::auth::router` is merged only
  when `[auth]` is configured, so the default auth-off deployment advertises two routes that 404.
- **`src/api_spec.rs:132-1515` — only `(path, verb)` is pinned; every row's status code, request
  schema, response schema and summary is hand-maintained**, despite the new docs/API.md section
  asserting the document is "generated, never hand-written".
  `every_served_route_is_documented_and_nothing_else_is` compares `(path, METHOD)` sets only.
  *Proof this is not theoretical:* the very next commit, 59bc2e6 (40 minutes later), had to
  hand-edit 536 lines of api_spec.rs because 20 PUT routes changed status — had the author
  forgotten, the suite would have stayed green with the document lying about every PUT. 59bc2e6 adds
  a classification table for PUTs; the other ~110 rows stay unpinned.
- **`src/api_spec.rs:1706-1733` — responses are success-only, and in places wrong.** The blanket
  `422` attaches only for `Json`/`JsonArray`/`Form`/`JsonFree` requests, so there is no `404` on any
  `GET /x/{id}` or `DELETE` (the summaries say "or 404" in prose only), no `422` on the multipart
  upload or the bare-text import feeds, no `502` on the import routes, no `500`. `POST /login` is
  documented as `303` only, although `login_submit` answers `200` re-rendering the page on bad
  credentials — a client written from the spec treats a failed login as an unexpected status.
- **`src/api_spec.rs:2164-2179` — the global money-as-string rule is pinned by one field of one
  schema** (`TradeBody.average_price`). Add a `#[schema(value_type = f64)]`, a future `f64` money
  field, or switch the utoipa feature to `decimal_float`, and the document advertises a JSON number
  for a tax figure while the suite stays green. The non-vacuous version walks
  `components.schemas` failing on any `"type": "number"` — `collect_refs` is already that shape.
- **`src/api_spec.rs:1867-1869` — the whole document (200+ schemas, ~130 paths) is rebuilt and
  re-serialised on every request**, and `components()` dedups with an O(n²) `unique.iter().find()`.
  A `LazyLock<String>` served with `content-type: application/json` builds it once; the dedup wants
  the `BTreeMap` the test at :2274 already uses. Same function: the collision check is a runtime
  `assert!` in a request path — it panics inside a handler (500 via catch-panic) for an invariant
  that test already pins.
- **`CLAUDE.md` Project structure was not extended for the new non-test `src/api_spec.rs`.** That
  section enumerates every `src`-root module; per the project's own doc-sync rule this is part of
  the same task. (It is arguably `src/infra/` material, being cross-cutting infrastructure.)
- `README.md:48-88` — docs/FEATURES.md gained a "Machine-readable API description" subsection but
  README's matching bullet was not extended, so the two feature lists disagree.
- Nit: two `ROUTES` rows with the same `(path, verb)` silently overwrite in
  `Paths::add_path_operation` and no test catches it — `expected` is deduped and
  `documented_routes` reads the collapsed map. A uniqueness assert costs three lines.

---

## 262e4e8 — Pin outbound money/quantity serialization

The two behavioural tests are **non-vacuous and do fail under `serde-float`** (verified against the
`rust_decimal` 1.43.0 source that `Cargo.lock` pins, including that `Trade` has no
`skip_serializing_if`, so the `statement_total` → `Null` assertion is real). No `f64` exists
anywhere in `src` and there is no hand-written `Serialize` impl, so the type-level codec does cover
the whole outbound surface today. The findings are about claims, coverage and reuse.

- **`Cargo.toml:17-21` — `serde-str` does nothing for the outbound direction, yet the comment
  presents it as what makes the wire format "a named choice rather than a side effect of
  `serde-float` happening to be off".** In `rust_decimal-1.43.0/src/serde.rs:533` the string
  `Serialize` impl is gated `#[cfg(not(feature = "serde-float"))]` — only. `serde-str` gates the
  `Deserialize` impls (:279, :289, :299) and nothing else. After this commit the outbound format is
  still exactly the side effect the comment says it no longer is, and the comment contradicts itself
  two sentences later. Failure: a maintainer trusts the declared codec, deletes
  `every_money_field_of_a_serialized_row_is_a_json_string` as "restating the manifest", and the sole
  outbound guard is gone. Relatedly `TODO.md` ticks "Add an explicit string codec (the write-side
  mirror of `strict_decimal`)" `[x]`, but no outbound codec was added — what landed is an *inbound*
  strictness feature plus tests. Either add
  `#[serde(serialize_with = "rust_decimal::serde::str::serialize")]` where it is wanted, or reword
  the manifest and TODO to say the outbound rule is pinned by test only.
- **`src/infra/decimal.rs:122-128` — the stated reason for not adding a serialize-side twin is
  wrong.** It says such a helper "would leave a `pub fn` reachable only from `#[cfg(test)]` code —
  which warns in the non-test build", but a `#[serde(serialize_with = …)]` on a real response struct
  is non-test code and `rust_decimal::serde::str::serialize` needs no local `pub fn` at all. The
  true reason is already in the first half of the comment; the invented CLAUDE.md justification will
  mislead the next person weighing the option.
- **`Cargo.toml:22-24` / `docs/API.md:1900` — `serde-arbitrary-precision` alone does not render
  responses as JSON numbers.** Both number-emitting `Serialize` impls (`serde.rs:544`, `:555`) are
  gated on `serde-float`; `serde-arbitrary-precision` changes only the `arbitrary_precision` helper
  module. Harmless in the manifest, but it is now a factual error pinned into user-facing API docs
  by `doc_checks`.
- **`src/infra/decimal.rs:739-770` — covers 5 of `Trade`'s 10 `Decimal` fields, and its final loop
  is vacuous under a rename.** The three `residual_*` DRP fields are untested despite the doc
  comment saying "eight", and `assert!(!obj[field].is_number())` passes silently when a field is
  renamed, because `obj[missing]` is `Value::Null`. Reuse: the *derived* form used in the second
  test (collect fields that serialized as numbers, assert equality with the id set) applied to
  `Trade` would be shorter and cover all 10.
- **Coverage gap the commit's own docs overclaim: the OpenAPI document is untouched.**
  `docs/API.md` now says the string rule "is enforced rather than incidental", but
  `GET /openapi.json` can still advertise a money field as `number`. Nothing pins `utoipa`'s
  `decimal` feature the way the new test pins `rust_decimal`'s. Residual: CLAUDE.md's "never `f64`"
  rule has no scan test anywhere — a money field typed `f64` defeats all of this.
- `src/infra/decimal.rs:810-817` — the expected-numeric list is transcribed, but fails *closed* (a
  new `i64` column fails with a confusing money-leak message), so not the forbidden silent mirror.
  Worth a line saying a new integer id belongs in the list.
- Nit: CLAUDE.md's Financial correctness section still describes only the inbound/TEXT-column
  direction and does not name the new outbound pin.

**Closes from earlier in REVIEW.md:** nothing. Specifically *not* the a8da1ad finding that the
OpenAPI money-as-string rule is pinned by one field of one schema — this commit pins the runtime
serialization of two Rust structs and does not touch `api_spec.rs`; all three defeats that finding
names still leave the document lying with a green suite.

---

## 9053adb — Document the error-body contract for machine clients

Core is sound: all ten `ApiError` variants are covered by the new media-type test, and the matrix's
variant→status/body mapping is correct line-by-line against `into_response` (`infra/http.rs:168-200`).
The `415`/`413` additions are right for axum 0.8 (missing content type → 415 text; the 2 MiB default
body limit → 413 text; `attachment.rs:192` raises its own route to 26 MB). `error_body_matrix_rows`
is a real parser, not a vacuous one.

- **`docs/API.md:1852-1865` + `src/api_spec.rs:65-72` — the GET-404 rule is factually wrong.**
  `GET /portfolio/activity?listing_id=…` with an unknown listing returns
  `ApiError::not_found(format!("listing {} not found", …))` (`src/reports/activity.rs:850-865`) —
  404 with `text/plain`. The matrix says a `GET`'s 404 is *empty*; the OpenAPI description is
  stricter still ("404 **on a delete or operation**" carries text). A client generated from
  `/openapi.json` sees the activity report's 404, concludes by contract there is no body, and
  discards "listing 42 not found" — exactly the loss this commit exists to prevent. Neither new test
  can catch it: both compare prose to prose, nothing probes the router.
- **`docs/API.md:1852-1867` — the new matrix and the `**Error bodies.**` paragraph now sit in the
  same section and contradict each other twice, both contradictions frozen by tests.**
  (a) The matrix row `| \`415 …\` | text |` says 415 carries a body, but the paragraph's code list
  omits it and `doc_checks.rs:796` `RETURNED_WITH_BODY` was not updated — so
  `error_bodies_list_names_only_returned_codes` `assert_eq!`s the list to the 7-code set and now
  **actively forbids fixing the doc**. Two tests in one file enforce mutually inconsistent contracts.
  (b) The paragraph still closes "the two `5xx`s that are not internal faults…", omitting `502`,
  which the matrix three lines above says carries text. This is REVIEW.md's 962ffaa finding
  verbatim: the commit *moved* that sentence into the new section and placed it directly under a
  table that refutes it, without fixing it.
- **`src/doc_checks.rs:922-931`, `src/infra/http.rs:858-903`, `src/api_spec.rs:2237-2245` — three
  more hand-transcribed mirror lists, none derived from `ApiError`.** Nothing forces a new variant
  into any of them; `into_response`'s match is exhaustive, the tests are not. This is the 962ffaa
  `RETURNED_WITH_BODY` finding tripled. *Proof it is live:* `a9da849` had to hand-edit all four
  lists for `TooManyRequests`/429 — it remembered, but nothing would have failed otherwise.
- **The four framework-produced rows are asserted nowhere: 404-empty-from-no-route, 405-empty,
  413-from-the-body-limit, and the newly claimed 415.** `415`/`UNSUPPORTED_MEDIA_TYPE` appears in
  `src` only in the two doc strings and the two tests that transcribe them. Swap a `Json<T>`
  extractor on some route and its no-content-type rejection becomes 400/422 while the documented 415
  contract stays green. `ApiClient::post_bytes(path, None, body)` already exists to test this.
- **`src/api_spec.rs:2232-2252` — the test is a verbatim copy of `DESCRIPTION` asserted against
  `DESCRIPTION`.** It proves nothing about the code or about docs/API.md; it catches only an edit
  that forgets to edit the test, and is a second copy that must move in lockstep. The missing
  cross-check — the description's status list vs `error_body_matrix_rows()` — is parseable in-tree.
- **`src/doc_checks.rs:946-953` — the new cross-check is one-directional**, reproducing the defect
  already logged under 962ffaa: every matrix status needs a Response-codes row, but not vice versa.
  (a9da849 had to bolt on a bespoke `429` assertion precisely because the structural one is absent.)
- The generated OpenAPI document gains the contract only as prose: no operation declares `415`,
  `413` or `405`. Extends a8da1ad's "responses are success-only" rather than creating it — but this
  is the commit whose stated audience is machine clients.
- Nits: `docs/API.md:1857`'s "405 … a write attempted on a read-only path" is narrower than reality
  (405 is any unmatched method, e.g. `GET /jobs/{name}`); `panic_response` is in the matrix but not
  the `cases` table though it returns a `Response` directly; the case table asserts content type but
  not that "empty" bodies are empty (folding in an expected-body column would collapse four
  single-variant tests into it); "2 MB" is axum's 2 MiB; and
  `error_body_contract_section()` is the ~tenth inline copy of the `## `-splitting idiom.

**Closes from earlier in REVIEW.md:** none fully. The 962ffaa `502` self-inconsistency is *partly*
addressed in spirit — the matrix now states correctly that 502 carries text — but the contradicting
sentence survives verbatim in the same section and is still pinned. Explicitly not closed:
`RETURNED_WITH_BODY` hand-transcription (made worse, and now wrong by one code, 415), the
`!HTTP_RS.contains("CONFLICT")` whole-file scan, the one-directional cross-check (replicated), and
a8da1ad's success-only responses.

---

## 0dfddd5 — Document list ordering, POST-for-read and pagination once

Good work, mostly verified correct: all 32 list endpoints checked against their `ORDER_BY` or
hand-written `ORDER BY`, the ordering table is complete (every `GET`-list route appears in exactly
one row), the POST-for-read set is exactly right at four, and every pagination claim checks out
against `reports/row_history.rs` (`DEFAULT_BROWSE_LIMIT = 100`, `MAX_BROWSE_LIMIT = 1000`, the
`limit+1` fetch, the 422s in the `row_id` form). Archiving is correct and verbatim.

- **`docs/API.md` ordering table — `/drp_enrolments` has the wrong key and the wrong group.** Its
  `ORDER_BY` is `"listing_id, holding_account_id, enrolment_date, id"`
  (`src/entities/drp_enrolment.rs:149`) — grouped by listing, then holding account, *then* date; the
  doc files it under "ascending date, then id" with key `enrolment_date`. A client merging
  enrolments across listings in date order reads them interleaved by listing and concludes rows
  changed. The one factual error in the table, and it survives to HEAD.
- **`src/doc_checks.rs:126-266` — the new pin is a hand-transcribed mirror of the doc table, not
  derived from `ORDER_BY`, which is exactly why the above passed.** It types 27 endpoint names into
  four loops and asserts each appears in the row it was already put in; nothing reads an `ORDER_BY`
  const or the router, and there is no completeness assertion, so a new list endpoint in no row
  fails nothing. `include_str!`-scanning `src/entities/*.rs` for `const ORDER_BY`, or the both-ways
  set comparison that already exists in-tree as
  `every_served_route_is_documented_and_nothing_else_is`, would be self-maintaining and would have
  caught it.
- **`docs/API.md` POST-for-read paragraph — "a `POST` on a write endpoint … is a write, not a read"
  is false for one operation.** `POST /amma_statements/{id}/generate_adjustments` with
  `"preview": true` writes nothing and answers 200
  (`src/entities/amit_adjustment_generation.rs:463-490`) — and the Response-codes table three
  paragraphs above says so. A client following "every POST is a write" refuses to call the preview
  it is supposed to call before confirming, or treats the 200 as having created adjustment rows.
- **`src/api_spec.rs:2276-2305` — third instance of a test asserting substrings of `DESCRIPTION`
  against `DESCRIPTION`.** `doc()` builds the document from that same const, so it can only fail
  when someone edits the const and forgets the copy in the test. Nothing cross-checks the compact
  twin against the long form, and the four-entry POST-read list now exists in three hand-maintained
  copies — only the third (`reports::tests::POST_BODIES`) is anchored to the router.
- **`docs/API.md` — "An entity list's key is its `CrudEntity::ORDER_BY`" is untrue of five endpoints
  it names.** `/exchange_holidays`, `/rights_sales`, `/attachments`, `/closing_prices` and
  `/listings/{id}/renames` use hand-written `ORDER BY` clauses with no const. A maintainer told
  where the key lives finds nothing, then changes the const-less query without touching the doc —
  the drift the section exists to stop.
- **`docs/API.md` — "the order is **total** (it always ends in a unique column)" overstates.**
  Several end in a unique *tuple*, not column: `/listings` ends in `ticker` (unique only with
  `exchange_mic`), `/rba_fx_rates` in `month`, `/exchange_holidays` in `holiday_date`,
  `/report_snapshots` in `report`. The stability guarantee does hold (UNIQUE constraints in
  migrations 0001/0021/0039/0048), but the parenthetical is the kind of claim a client tests.
- `/exchange_holidays` is filed under "ascending date" though its lead key is `mic` (the
  parenthetical states the truth, so a careful reader is fine).
- Low: "entity lists are not paginated" is a scope decision stated only in the new section, not in
  Known limitations as CLAUDE.md requires. (9400e70 later bolted on a cross-ref, so the gap was real.)
- `src/doc_checks.rs:59-67` — **eleventh** inline copy of the `## `-splitting idiom.
- Nits: the `**Order.**` paragraph opens "Every list endpoint returns its rows **ascending**" then
  names five that do not; `DESCRIPTION` gains literal `\n\n` breaks in a const that was previously
  one `\`-continued run.

**Closes from earlier in REVIEW.md:**
- **41e808a — the wrong "12 POST reads" count: closed** for the live docs. The new section and the
  OpenAPI description both state the correct post-ea21d55 figure of four, verified against the
  router; the wrong count survives only inside the verbatim `DONE/api.md` archive, as the
  never-delete rule requires.
- **ea21d55 — the POST-for-read set discoverable only per-section: partly closed.** Now stated once
  with each body's reason — but in three hand-maintained copies.
- Not closed, and extended: the hand-transcribed-mirror-list theme (two more added), the
  doc-against-doc cross-check theme, and the section-splitting reuse item.

---

## 59bc2e6 — Report a PUT's create-vs-replace outcome

**Strong commit.** The atomicity story — the thing most likely to be got wrong — is correct
everywhere: all 20 PUT routes are accounted for, and every `existed` check runs on the write's own
`write_tx` (`BEGIN IMMEDIATE`) connection before the INSERT. Eight handlers that previously wrote on
a bare pooled connection (`exchange`, `exchange_holiday`, `cgt_settings`, `tax_year_settings`,
`interest_income`, `investment_expense`, `holding_account`, `amit_adjustment`) were moved onto a
`write_tx` — a real integrity improvement beyond the stated scope. The two exceptions
(`/transfers/{id}` create-only 201, `/rba_fx_rates/{id}` correction-only 204) are correctly left
alone, and docs/API.md was updated thoroughly.

- **`src/web/app.js:703` — the UI throws away the very signal this commit added, on the one path
  where the clobber actually happens.** For a natural-key entity (`exchanges`, `exchange_holidays`,
  `tax_year_settings`, `cgt_settings`) the **Create** form PUTs the key the user typed. If that key
  is already taken the server now answers `204 Replaced` — and `app.js:704-706` toasts "Saved."
  regardless. A user adding exchange `XASX` on the create form silently overwrites the seeded XASX
  row's name/timezone/settlement_days, changing T+n settlement on every ASX trade, with the UI
  reporting success. That is exactly the scenario the commit message describes; the server half is
  fixed, the client half is not. Symmetrically, the edit branch (`app.js:680`) gets a `201` when the
  row was deleted under the user and silently resurrects it. The new `web.rs` test only pins that
  `api()` *accepts* both statuses.
- **`CLAUDE.md:71` — the API-conventions rule was not updated and is now false.** It still reads
  `PUT /entities/:id → 204 No Content (upsert)`. This is the file the next entity author writes
  against: a new entity written to `Ok(StatusCode::NO_CONTENT)` per the stated convention fails
  `every_put_route_reports_create_then_replace` only after being added to `PUT_ROUTES`, and a
  reviewer arbitrating between rule and code has the rule on their side.
- **`TODO.md:25-31` — ticked `[x]` although half its stated acceptance test was deliberately not
  implemented, with no note recording the decision.** The Test line is "round-trip tests assert the
  create-vs-update signal; **an `If-Match`-stale write answers `412`/`409`**"; the second test does
  not exist, and the commit instead states in `docs/API.md:98` that there is "deliberately no
  `If-Match`/ETag". The decision is defensible — REVIEW.md's 41e808a section shows the If-Match half
  would have collided with 962ffaa's 409-removal pin — but the reasoning lives only in API.md prose,
  so the archived item will read as if `412` shipped.
- **`src/api_spec.rs:2347-2354` — the OpenAPI pin is a second hand-transcribed copy of
  `entities::tests::PUT_ROUTES`, not a read of it.** It re-declares its own `EXCEPTIONS` and its own
  `puts == 20`, while `PUT_ROUTES`'s doc comment claims it "pins the same classification in the
  generated OpenAPI document". Reclassify `/transfers/{id}` in `PUT_ROUTES` alone and api_spec's
  copy still asserts `&[201]` and passes — the document lies while both tests are green. `PUT_ROUTES`
  lives in a private `mod tests`, which is why it was copied; hoisting it to `infra::http` or
  `test_support` behind `#[cfg(test)]` makes both tests read one list.
- **`src/infra/http.rs:200-210` — the create-vs-replace *decision* is atomic but the `201` body is
  not.** `upsert_response` calls `crud_get(pool, key)` after the transaction commits (same in
  `exchange_holiday.rs:259-261` and `closing_price/http.rs:187`). A concurrent DELETE in that window
  turns a committed create into `ApiError::internal("the row a PUT just created could not be read
  back")` — a 500 with an empty body for a write that succeeded; a concurrent replace makes the
  `201` report *the other writer's* row, contradicting `docs/API.md:98`'s promise that the body is
  what this write stored. Every one of these handlers already holds the row inside its `tx` a few
  lines earlier, so reading it there is both correct and one round-trip cheaper.
- `exchange_holiday.rs:259` and `closing_price/http.rs:187` read the row back even on `Replaced`,
  where `upsert_response_of` discards it — a wasted query on the common path, and the one asymmetry
  with `upsert_response`, which early-returns. The `(Created, None)` → 500 arm has no test;
  `infra/http.rs` gained 122 lines of non-test code and no unit test of its own.
- `src/api_spec.rs:887-892` — `PUT /rba_fx_rates/{id}`'s summary now asserts "Always 204" while the
  handler also answers `404`, which the document still does not carry. A new instance of a8da1ad's
  success-only responses, sharpened by the absolute wording.
- Nits: `docs/API.md:98` says the `201` body is "exactly the body `POST /<collection>` answers", but
  five of the eighteen routes have no collection POST (they answer 405 by design, as the same
  section says two paragraphs later). The "deliberately no If-Match/ETag" scope decision belongs in
  Known limitations. No `Location` header on the 201s — not required for PUT, not counted a defect.

**Closes from earlier in REVIEW.md:**
- **a8da1ad — hand-maintained `ROUTES` statuses (and the prediction that this commit would hand-edit
  536 lines): partly closed.** `RouteRow`'s status became `&'static [u16]`, and every PUT row is now
  pinned twice — structurally by `every_put_route_documents_its_outcome` (which also checks the
  document carries a body on 201 and none on 204) and behaviourally by
  `every_put_route_reports_create_then_replace`, which drives all 18 routes through a real
  create-then-replace. The 20 PUT rows can no longer silently lie. The rows were still *edited*, not
  derived: the other ~110 statuses and every row's schema/summary remain unpinned.
- **a8da1ad — responses are success-only: partly addressed** for PUT; the missing 404/422/500 arms
  are untouched.
- **41e808a — the two items that break each other's test: closed in effect.** B3 was implemented via
  the status-signal branch of its own "and/or", with the no-If-Match decision pinned by
  `doc_checks::creating_a_record_documented`, so the collision can no longer land. Not recorded in
  TODO/DONE, which is the third finding above.

---

## 9400e70 — Filter the workhorse entity lists server-side

**Strong, careful commit.** `crud_list_filtered` builds `SELECT {COLUMNS} FROM {TABLE} WHERE 1=1`
from `&'static str` constants only, every filter *value* goes through `push_bind`, and
`ORDER BY E::ORDER_BY` is appended unchanged — **no SQL injection vector** in any of the 14 filter
implementations, and the documented total ordering is preserved. `CrudEntity::Filter` as an
associated type with `apply_filter` on the filter (rather than a defaulted method on `CrudEntity`)
is a genuinely good choice: an accepted-but-ignored parameter is not expressible. All 22 `TABLE`
consts are plain names; the 14 narrowing tests are non-vacuous and genuinely distinguish AND from OR.

- **The owner filters silently drop rows with a NULL owner, and nothing a client reads says so.**
  `investment_expenses.listing_id`/`holding_account_id` are nullable by design
  (`migrations/0001_schema.sql:498-501`: "Both NULL for a portfolio-wide expense (e.g. an adviser's
  whole-of-portfolio fee)"), as is `interest_income.holding_account_id`. `apply_filter` emits
  `AND holding_account_id = ?`, so those rows match no value. A machine client totalling deductions
  by iterating `GET /investment_expenses?holding_account_id=N` over the accounts omits the
  whole-of-portfolio adviser fee entirely — **the deduction figure is understated with no error**.
  The author knew: both new tests carry a comment saying so. But it lives only in `#[cfg(test)]`
  prose, not in the Filters table, the per-endpoint row or the OpenAPI summary, and every other
  filtered column is NOT NULL, so a client cannot infer the exception.
- **Two list routes still ignore every query parameter, and the new section doesn't say so.**
  `rights_sale.rs:727` and the `/exchange_holidays` list decode no query string, so
  `GET /rights_sales?listing_id=3` answers 200 with the whole table. The section names the
  unfiltered lists as exactly seven reference/settings tables and bolds that an unrecognised
  parameter is refused 400 "on every list route that decodes a query string" — true, but neither
  route appears anywhere in the section. Rights sales are CGT events carrying a listing, i.e.
  exactly what a client will try to filter. `LIST_ROUTES` classifies them honestly as
  `HandWrittenIgnoringQuery`; that reason belongs in the user-facing doc.
- **`src/entities/mod.rs` — `shared_list_route_paths()` fails *open*.** The needle is the literal
  `"get(http::list_handler::<"`; an entity that does `use crate::infra::http::list_handler;` and
  registers `get(list_handler::<X>)` is invisible to the scan **and** absent from `LIST_ROUTES`, so
  the both-ways `assert_eq!` still passes and the list ships unclassified — the exact drift
  `every_list_route_is_classified_for_filtering` exists to stop. The `scanned.len() >= 20` floor
  catches a total parse failure, not a single miss.
- **`src/infra/http.rs` — the bind-not-interpolate rule is enforced by a doc comment only.**
  `apply_filter` takes a raw `QueryBuilder`, so `qb.push(format!(" AND ticker = '{v}'"))` compiles
  and passes every test in the tree. This repo enforces the analogous rules structurally
  (`never .bind(x.to_string())`, `write_side_modules_never_begin_a_deferred_transaction`). No defect
  today; a scan over `impl CrudListFilter` bodies, or a `push_eq`/`push_date_range`-only API, makes
  it a fact rather than a convention.
- **`src/api_spec.rs` — the OpenAPI document still declares no query parameters; this commit adds 16
  more filtered routes as English only.** Every filter lives in a `summary` string; no `*ListQuery`
  derives `IntoParams`/`ToSchema`. The two new tests compare prose to prose, so adding, renaming or
  dropping a filter fails nothing. The filters are now named types per entity — exactly what
  `IntoParams` consumes — so the self-maintaining version is cheap here.
- `every_filtered_list_summary_names_its_filters` is another hand-transcribed mirror with an
  overclaiming message: `assert_eq!(checked, 16, "every filtered list route must be named here, and
  no other")` counts only the rows the test typed in, and `/attachments` is pinned on `?trade_id=`
  alone though it takes seven parameters. `doc_checks::list_filtering_contract_documented` adds a
  third hand-maintained copy of the same table.
- **`TODO.md:33` ticked although its acceptance criterion was deliberately not met**, with no note:
  the Test line says unknown params "still `422`", the implementation answers `400` — correctly, per
  the documented unreadable-query-string status. Once archived the item reads as though 422 shipped.
- Observation, not a defect: `src/web/*` is untouched, so `filterableTable` still fetches whole
  tables and filters client-side. No duplication or disagreement results — but there is no consumer
  either, and `ENTITIES` has no way to declare a server-side filter, so the capability is API-only.
  (Verified no entity list is fetched with a query string, so nothing regressed to a 400.)
- Nits: the Known-limitations bullet says "filters added 2026-09-24" but the commit is dated
  2026-09-25; the consolidated table drops `/report_snapshots/series?listing_id=` and
  `holding-series?from=&to=`, which the sentence it replaced named; string filters are byte-compared
  (no `NOCASE`), so `?exchange_mic=xasx` matches nothing, undocumented.
- **Simplification:** the `listing_id`/`holding_account_id`/`from`/`to` struct-plus-`apply_filter`
  block is written out verbatim across ~10 entities — ~200 near-identical lines differing only in
  the date column name. Two helpers in `infra::http` (`push_eq`, `push_date_range`), or a macro
  generating the struct + impl from `(name, date_column, owners)`, would collapse it, put "the
  column name is a trusted constant" in one place instead of fourteen, and give the bind-rule
  finding above its single choke point.

**Closes from earlier in REVIEW.md:**
- **0dfddd5 — the pagination scope decision missing from Known limitations: CLOSED.** The
  `**Server-side pagination**` bullet now records that filters landed, names the parameters,
  cross-links `#reading-a-list`, and states that cursor paging is what remains open; both halves are
  pinned.
- **ea21d55 — the 400-vs-422 boundary pinned for only 3 of 7 moved reads: not closed** (those are
  report reads) — but the same boundary is now behaviourally driven across all 23 query-decoding
  list routes by `every_list_route_refuses_an_unknown_parameter`, the strongest instance of that pin
  in the tree and the model the four unpinned report reads should follow.
- **a8da1ad — no query parameters in the OpenAPI document: not closed, extended** by 16 routes.
- The hand-transcribed-mirror theme: **extended by two more lists.**

---

## 8da9424 — Expose as_of_date on the valuation reports and state its default

**Careful, largely correct plumbing + docs.** The as-of machinery was already right; this exposes it.
Verified end to end: `domain::open_parcels::load` (`src/domain/open_parcels.rs:192`) resolves `None`
via `as_of_or_today`, never `as_of_or_open`; the cutoff is threaded to the parcel SELECT,
`db_units_sold`, `amit_adjustment::db_cost_base_reduction_events`, `cost_base::Held::AsAt` and
`split_adjusted_quantity`, so the "inclusive bound / that date's unit basis" claims are true and the
new boundary tests are non-vacuous. `as_of_or_today` vs `as_of_or_open` is correct at all four
handlers. Using a Crypto listing to dodge the exchange calendar is a sound test choice.

- **`src/reports/portfolio.rs:186-215` + `src/web/config.js:483` + `docs/API.md:1221,1254` — a
  back-dated `as_of_date` is combined with *today's* live quote, and the new docs assert the
  opposite.** `overview` bounds the holdings at `as_of`, then calls
  `closing_price::resolve_live_prices`, which returns the provider's latest quote and converts via
  `resolve_valuation_rate` at `quote.as_of.date_naive()` (`live.rs:82-91`). This commit adds an
  "As-of date" input to the Overview screen (`asOfDate: true`) while `app.js:3087` hardcodes
  `live: true` — so entering `as_of_date: 2024-06-30` yields 2024 quantities times today's price and
  today's FX (possibly `fx_provisional`), labelled as the position at 2024-06-30. The new As-at
  paragraph states the date bounds the report with no carve-out; the live path's exemption is stated
  nowhere. Pre-existing on unrealised-gains/performance (performance's `docs/API.md:1395` "market
  value at `as_of_date`" is outright false under `live`), but this commit generalises the claim to
  all four and adds the control that makes it easy to hit. One sentence in the Live-valuation
  section, or refusing `live` with a past `as_of_date`, closes it.
- **`src/reports/open_parcels.rs:60-70` / `src/api_spec.rs:1313-1318` — the new query parameter is
  in the OpenAPI document as English only.** `OpenParcelsQuery` derives neither `IntoParams` nor
  `ToSchema`; the new test at `api_spec.rs:2757` pins only that the *summary prose* contains
  `"?as_of_date="`, so it looks like coverage while the document stays silent and a generated client
  cannot pass the date. (The three POST bodies *are* genuinely pinned — the test reads
  `components.schemas.*.properties.as_of_date` and asserts it is not `required`.)
- **`src/web/config.js:495` — Open Parcels is the one of the four "valuation reports" whose UI does
  not offer the date**, though `docs/API.md:1221` names all four and the generic `params:` mechanism
  is exactly the supported way to declare it. A user reconciling a broker statement as at 30 June
  must hand-craft a URL. Also `web.rs`'s new `overview_report_offers_the_as_of_date` pins the config
  line as one brittle ordered substring, so a harmless property reorder breaks it.
- **`TODO.md:39-44` ticked while one named sub-target was left undone, with no note.** The item
  names `parcel-optimiser` among the reports that "each default their own";
  `src/reports/parcel_optimiser.rs:470` still does
  `.unwrap_or_else(|| chrono::Local::now().date_naive())` rather than going through `infra::date`.
  The deliberate `sale_date` ≠ `as_of_date` name split is well argued, but that is an argument about
  the parameter *name*, not about re-deriving "today" inline for the fourth time in the tree
  (`health.rs:3271`, `doc_checks.rs:3675` are the others). Once archived it reads as if all four were
  unified.
- **`src/doc_checks.rs:3961-4010` — doc-against-doc, and its last two assertions duplicate the test
  three lines above.** `as_of_date_is_the_documented_valuation_date` asserts only that API.md
  contains strings API.md was edited to contain; its closing block, commented "Each report's own
  route row names the parameter and the default", re-asserts two strings against the *whole*
  `API_MD` that `as_at_today_convention_documented` (edited by this same commit) already covers, and
  is not section-scoped despite the comment. Delete the doc change and both fail together; change
  the *behaviour* and neither notices. Also the ~12th copy of the section-splitting idiom.
- Low / simplification: `open_parcels.rs:78-92,169-179` — the default is now resolved twice (handler
  *and* loader), and `db_open_parcels`'s `None` branch is reachable only from the ~35 test call sites
  this commit had to edit across 13 files. Passing `q.as_of_date` straight through is behaviourally
  identical with no churn. The API default is now stated in three places, so a change to the
  loader's resolution is masked by the handler.
- Nit: `docs/API.md:1280`'s fence is `GET /portfolio/open-parcels?as_of_date=2026-06-30` with no
  indication the parameter is optional; every other optional-body report shows the bare path plus a
  separate "(optional)" line.

**Closes from earlier in REVIEW.md:** none fully — no earlier item names `as_of_date` or
`infra::date`. Worth recording as partial credit: the commit removes two of the four inline
`chrono::Local::now().date_naive()` defaults in favour of the shared helper, the kind of
consolidation this review keeps asking for elsewhere. Extended, not closed: a8da1ad's missing query
parameters (one more route) and the doc-against-doc theme.

---

## a80370b — Add a ?status=ok|error filter to GET /closing_prices

**Substantially clean**, and the two traps this range had been accumulating were both avoided:
`status` is the existing typed `PriceStatus` enum (`closing_price/model.rs:11-18`), and the column
is `TEXT NOT NULL CHECK (status IN ('ok','error'))` in every schema revision — so **the 9400e70
NULL-owner trap cannot apply**: there are no NULL rows for `AND status = ?` to drop and the two
values exhaust the CHECK. No injection (`push_bind` via the `sqlx::Type` derive).
`deny_unknown_fields` is kept, and the documented `400` really does name the parameter — axum 0.8
wraps the deserializer in `serde_path_to_error`, so the body reads ``status: unknown variant
`maybe`…``, making the test's `contains("status")` real rather than accidental. Every hand-maintained
mirror was updated (`LIST_ROUTES`, api_spec's `FILTERED` list, `list_filtering_contract_documented`,
both API.md sites), and `api_list_filters_by_status` is genuinely non-vacuous.

- **`src/api_spec.rs:925`, `:2624-2630` — the new parameter reaches the OpenAPI document as English
  only, and the test that looks like coverage pins prose against prose.** `ListParams` derives
  neither `IntoParams` nor `ToSchema`; a client generated from `/openapi.json` cannot send
  `?status=`, so it still fetches both kinds and drops the nulls — the exact cost this commit exists
  to remove. Nothing fails if the parameter is renamed or dropped.
- **`src/entities/closing_price/db.rs:82-88` — `db_list` was made `Executor`-generic with a
  rationale no caller exercises.** The doc says it is "Executor-generic so it composes onto a
  caller's own connection the way `db_get_one` does", but the only non-test caller
  (`http.rs:108`) passes `&pool`, as do all seven test sites. Out-of-scope generality that forced a
  diff across three test files, and the comment reads as though a composing caller exists.
- **`docs/API.md:302` — "so a client computing a valuation gets the priced series in one call" is
  over-claimed.** `?status=ok` also returns rows **superseded by the listing's `unpriced_before`** —
  `status: "ok"` with a price, but read by no valuation (`app.js:1799-1803` computes `_superseded`
  for exactly this reason). The paragraph carefully closes the errored direction but says nothing
  about the superseded one, so a client told it has "the priced series" values days the server
  excludes. One half-clause closes it.
- `src/doc_checks.rs:~3208-3217` — `closing_price_status_filter_documented` re-asserts the Filters
  row that `doc_checks.rs:345` (edited by this same commit) already pins, and its own comment admits
  it. All six assertions compare API.md prose to API.md prose. The behavioural half is covered
  separately, so this is duplication rather than a gap.
- `src/api_spec.rs:2563-2566` — fourth instance of a test asserting a substring of `DESCRIPTION`
  against `DESCRIPTION`; the 60-word clause now exists twice, in lockstep, in one file.
- `src/entities/mod.rs:1099` — `sample_filter_value` gains a globally-keyed `"status" => "ok"`. The
  map is keyed on the bare parameter name across all 23 list routes, so a future route with its own
  `status` over a different value set silently gets `"ok"` driven at it; the test asserts only
  `200`, so that route's filter goes untested. Same shape as the existing `"security_type" =>
  "Share"` — pre-existing design, newly extended.
- `closing_price/tests/fetch.rs:611` builds `ApiClient::over(router().with_state(pool))` inline
  rather than the module's own `full_router` helper, which the near-identical sibling test 30 lines
  above uses.
- **Simplification (extends 9400e70):** `db_list` is now a fifth hand-written
  `qb.push(" AND x = ").push_bind(v)` pair in one function. The proposed `push_eq`/`push_date_range`
  helpers would cover all four filters here — `/closing_prices` is a hand-written list with no
  `CrudEntity` impl, so it sits entirely outside the `CrudListFilter` machinery 9400e70 built.
- Nits: a `doc_checks` assertion message reads "the Closing prices line table…" (stray "line"). No
  `config.js` change was needed and none was made (the screen is `custom: 'prices'` and deliberately
  reads every row); no new `sel()` list, so nothing to classify in
  `select_option_lists_are_pinned_to_their_server_side_source` — verified, not an omission.

**Closes from earlier in REVIEW.md:** none. Worth recording as the mirror image: **9400e70's
NULL-owner trap is correctly not repeated** — this is the first filter added since that finding and
the column is NOT NULL + CHECK-constrained, so the failure mode cannot recur. Extended, not closed:
a8da1ad's missing query parameters, the hand-transcribed-mirror theme (four lists, all four
remembered this time), and the doc-against-doc theme.

---

## a9da849 — Lock out repeated failed logins per source

High-quality, carefully documented work. The lockout is gated **before** the Argon2 verify (so a
correct password cannot reset an active lockout), the key is the IP not IP:port, the table is
hard-capped and pruned, success clears the streak, `Retry-After` is set by `into_response` rather
than per handler, refusals log at WARN naming the source and never the password, and the
attacker-chosen username is Debug-quoted against log injection. The tests genuinely drive the whole
budget→429→cooldown→clean-slate cycle over HTTP. Docs, README, FEATURES, the OpenAPI `429` and the
DONE archive are all done properly. None of the findings below is "the lockout doesn't work".

- **`src/infra/auth.rs:818`+`:838` — the budget is check-then-act, so it does not bound *concurrent*
  guesses.** `lockout_retry_after` takes and releases the mutex, then ~30 ms of Argon2 runs, then
  `record_login_failure` takes it again. 500 parallel wrong passwords from one IP all pass the gate
  before the counter reaches 5: the effective budget is `5 + concurrency`, repeatable every
  cooldown. `docs/API.md:78`'s "a single-credential deployment cannot be brute-forced online"
  overstates what is enforced. A reservation (increment in-flight at gate time, decrement on
  completion) closes it. **Amplifying corollary:** `verify_password` runs on the async handler with
  no `spawn_blocking`, and `Argon2::default()` is m=19 MiB — so those same 500 logins are ~9.5 GB of
  transient allocation and 500 blocked tokio workers on an *unauthenticated* route. The
  blocking-in-async part is pre-existing; framing this as the brute-force control is what puts it in
  scope.
- **`src/infra/auth.rs:164` — `Source::Peer(IpAddr)` uses the full IPv6 address, so an IPv6 attacker
  defeats the lockout for free.** A residential/VPS allocation is a /64; rotating the low 64 bits
  per attempt costs nothing, gives a fresh 5-attempt budget each time, and evicts real entries via
  the 4096 cap. The module doc's justification — "it costs real work — a distinct peer address per
  attempt" — is true for IPv4 and false for IPv6. fail2ban and nginx `limit_req_zone`, cited in the
  same comment as precedent, key IPv6 on the **/64 prefix** for exactly this reason. No test covers
  an IPv6 peer at all.
- **`src/main.rs:167-180` / README — in the *documented recommended* deployment the control converts
  a brute-force risk into a trivial denial-of-sign-in.** Behind the nginx reverse proxy the peer is
  always 127.0.0.1, so there is one global bucket: 6 bad logins every 5 minutes locks the owner out
  indefinitely, from anywhere. Disclosed honestly in three places, which is why it is not first —
  but the mitigation offered (nginx `limit_req`) limits the attacker's rate, not the shared bucket,
  and the docs don't state the practical escape (the bearer path is not locked out, so scripted
  access still works). An *opt-in* `trusted_proxy`/`X-Forwarded-For` setting is not the forgeable
  design the docs reject. Sub-effect: in that one bucket a legitimate success wipes the attacker's
  streak, further weakening the bound.
- **`src/infra/auth.rs:831` — sending `Accept: text/html` suppresses the `429` entirely.** The
  refusal still happens (200 + sign-in page), so this is not a bypass — but the machine-visible
  signal an operator builds on (a 429 in the access log, a fail2ban rule, an alert) never fires for
  an attacker who sets one header, which is the first thing a brute-force tool does. Browsers render
  a body fine on a 4xx.
- **`src/infra/auth.rs:131`/`:275`/`:395` — the failure window is a fixed window from the first
  failure, not the idle window the code comment and docs describe, and it is untested.**
  `window_started` is never refreshed on subsequent failures, so a source failing every minute still
  has its streak dropped at the 15-minute mark — whereas the doc says the streak survives "with no
  further attempt". No test drives `LOCKOUT_FAILURE_WINDOW` expiry, so neither the documented nor
  the actual semantics is pinned.
- `src/doc_checks.rs:5910` — the cooldown pin is ambiguous:
  `AUTH_RS.contains("Duration::from_secs(5 * 60)")` matches any const with that value. Swap cooldown
  and window and the test stays green while the docs' "5 minutes" is wrong. The budget and cap pins
  name their consts; `LOCKOUT_FAILURE_WINDOW` is pinned nowhere.
- `src/infra/auth.rs:351-353` — the comment says "Rounded up" but `as_secs()` truncates: with 299.4 s
  left the header says `299`, so a well-behaved client retries and is refused again.
- **`CLAUDE.md`'s `infra/auth.rs` bullet was not updated** for the lockout or for `main` now serving
  `into_make_service_with_connect_info` — the same doc-sync omission logged against a8da1ad.
- Nits: `prune` is O(n) over 4096 entries under one global mutex on *every* login attempt, reachable
  unauthenticated; the bearer and cookie paths are not rate-limited at all and the module doc never
  states the exemption; two tests sleep (~2.75 s added to a ~6.8 s suite);
  `doc_checks.rs:5905`'s `!config.rs.contains("lockout")` would fail on a mere doc comment.

**Closes from earlier in REVIEW.md:** none fully.
- **962ffaa/9053adb's four hand-transcribed mirror lists — not closed, and this commit is the
  predicted proof.** It had to hand-edit `RETURNED_WITH_BODY`, the matrix `EXPECTED`, `infra/http.rs`'s
  `cases` table and api_spec's `DESCRIPTION` copy. It **got all four right** — but nothing would have
  failed had it missed one.
- **962ffaa's "the two `5xx`s that are not internal faults" sentence — still wrong, still pinned.**
  This commit rewrote the *end* of that very sentence to append the 429 clause and left the wrong
  count standing. Third commit in a row to touch it without fixing it.
- **9053adb's 415-vs-`RETURNED_WITH_BODY` contradiction — still open.** Adding 429 to both sides
  without touching 415 leaves the two tests mutually inconsistent exactly as logged.
- 962ffaa's `CONFLICT` whole-file scan and the one-directional cross-check: untouched — this commit
  again bolts on bespoke per-status assertions (`doc_checks.rs:5875-5885`) precisely because the
  structural cross-check is absent.
- a8da1ad's missing `securitySchemes` is untouched and now more conspicuous: the document describes
  a `429` lockout on a login route while not describing the authentication that route exists for.
  Its missing query parameters, `servers` and success-only responses are likewise untouched — though
  the new `429` is the first non-success response any operation carries, so the pattern for fixing
  the rest now exists in `operation()`.
