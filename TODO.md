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

The open work below is what the 2026-09-26 per-commit review of that audit
deliberately left, having fixed everything else; the review itself is archived in
[`DONE/reviews.md`](DONE/reviews.md).

## Per-commit review of the REST API audit — deferred items (2026-09-26)

Every other finding of that review was fixed (commits `7c5aed1`, `c91dfb3`, and the
fifteen before them). These were judged out of proportion to the task at hand, or
need a design decision first. Each says why it was left, so a later pass can weigh
it rather than rediscover it.

- [x] Enforce *"money and quantities are always `Decimal`, never `f64`"* with a scan
      test. It is the one rule in CLAUDE.md's Financial correctness section with no
      test behind it: no `f64` exists in `src` today, and the type-level codec is what
      makes the outbound money-as-string pin structural, so a money field typed `f64`
      would defeat both at once — and nothing would fail. `infra::decimal`'s existing
      `.bind(x.to_string())` scan is the shape to copy. Test: the scan itself, over
      `src`, with the deliberate non-money `f64`s (if any) allowlisted with reasons.
      Done: `infra::decimal::tests::no_source_file_types_a_money_or_quantity_as_a_float`
      scans every `.rs` under `src` for `f64`/`f32` as a whole token, over
      `test_support::code_only` — a small Rust lexer that blanks comments and string /
      char literals while keeping line numbers, since the rule is quoted in prose all
      over the tree (the OpenAPI `DESCRIPTION` spells it out across a dozen
      `\`-continued lines that a line-by-line comment trim reads as code). The only
      floats in `src` are the two `visit_f64` arms that *refuse* a JSON number, which
      `FLOATS_ALLOWED` names with that reason — and an entry matching nothing fails the
      test, so the scan cannot go vacuous. `code_only` is itself pinned by
      `code_only_keeps_code_and_drops_prose` beside it.
- [x] Make `CrudListFilter`'s bind-not-interpolate rule structural. `apply_filter`
      takes a raw `QueryBuilder`, so `qb.push(format!(" AND ticker = '{v}'"))` compiles
      and passes every test; all ~14 filters go through `push_eq`/`push_date_range`
      today by convention only. Either scan `impl CrudListFilter` bodies for a `push(`
      whose argument is not a literal, or narrow the trait so a filter can only reach
      the helpers. This repo enforces its comparable rules structurally
      (`write_side_modules_never_begin_a_deferred_transaction`), which is the argument
      for doing it. Test: the scan, or the narrowed signature refusing to compile.
      Done, by narrowing rather than scanning: `apply_filter` now takes an
      `infra::http::FilterClauses<'_>` whose `QueryBuilder` is private, exposing only
      `.eq` / `.date_range`. A filter has no method that reaches the SQL text, so no
      scan and no allowlist are needed, and `qb.push(format!(…))` inside an
      `apply_filter` no longer names anything. The free `push_eq`/`push_date_range`
      helpers are gone; the two hand-written filtered reads that also used them
      (`closing_price::db_list`, `reports::snapshot`'s two date-ranged reads) go through
      `FilterClauses::over`, so there is one implementation of the binding rather than
      one inside the trait's reach and one outside it. The behavioural half is
      `entities::tests::a_filter_value_is_bound_not_interpolated`, over `exchange_mic`
      (the only free-text filter): a value closing the quote and opening an `OR` matches
      no row, and the table a `DROP` names is still there afterwards.
- [x] Derive the OpenAPI route table's remaining hand-maintained fields. `(path, verb)`
      is pinned both ways, every PUT row's statuses are pinned twice, and the query
      parameters now come from each route's `Query<T>` type — but ~110 rows' success
      statuses, request/response schema names and summaries are still typed by hand.
      The proof this drifts is in the archive: `59bc2e6` had to hand-edit 536 lines when
      the PUT statuses changed, and nothing would have failed had it not. Deriving them
      needs handler metadata the tree does not carry (a per-handler attribute, or
      `utoipa::path` on every handler), so this is a design decision, not a fix.
      Test: a both-ways comparison per field, as `every_served_route_is_documented_and_nothing_else_is`
      already does for `(path, verb)`.
      Done, and the design decision it needed turned out not to be a new per-handler
      attribute: the handler's own **types** already say all of it, so four scans read
      them rather than anything being declared twice.
      `the_generic_crud_routes_derive_their_status_and_schemas` takes the status and
      response of all ~55 `http::list_handler`/`get_handler`/`delete_handler`
      registrations off the type parameter (`list_handler::<Listing>` *is*
      `JsonArray("Listing")`). `every_request_body_matches_its_handlers_extractor` and
      `every_response_body_matches_its_handlers_return_type` follow every other
      registration to its `fn` and read the `Json<T>`/`Form<T>` out of the parameter
      list and the `Json<T>`/`Json<Vec<T>>`/`UpsertResponse<T>`/`StatusCode` out of the
      return type — both directions, with `#[schema(as = …)]` honoured. The five
      hand-built `Response`s (two CSV exports, the stylesheet, the attachment download
      and upload) are classified in `RESPONSES_NOT_DERIVABLE` and the five closure
      registrations in `UNRESOLVED_HANDLERS`, each with what it answers, so the lists are
      exhaustive rather than a sample. `the_success_statuses_are_derived_from_the_verb_or_the_handler`
      covers the statuses: a GET's 200 and a DELETE's 204 from the verb (110 of the 171
      rows), a POST's from the `StatusCode::…` its handler names, and the PUTs from
      `entities::PUT_ROUTES` as before.
      The scans compose `test_support::code_only` over `api_spec`'s `strip_test_modules`,
      both now blanking byte-for-byte, so a token found in the code can be read back out
      of the raw source at the same offset — which is how the route path, a string
      literal, is recovered.
      Two things stay hand-written, by decision: the **summaries**, which are prose no
      scan can write (their `?name=` halves are already cross-checked against the real
      `Query<T>`), and the `POST /login` row's `Form("LoginForm")`, whose handler decodes
      the form inside its body so it can answer either HTML or plain text.
      The derivation also found a real defect on its first run: `POST /rba_fx_rates/import`
      documented its response as `RbaImportSummary` while the handler returns
      `ImportOutcome` — the summary *plus* the provisional-snapshot true-up that
      `docs/API.md` has always described. `ImportOutcome` is now a `ToSchema`
      (`RbaImportOutcome`) and the row records it.
- [x] Decide the four `api_spec` pins that assert `DESCRIPTION` against `DESCRIPTION`
      (`the_two_global_rules_…`, the ordering/pagination half of
      `the_list_reading_contract_…`, `the_put_outcome_rule_…`,
      `the_list_filtering_contract_…`). Each checks that a documentation *requirement*
      is met, and the facts they describe are pinned behaviourally elsewhere — so they
      are not worthless, but they cannot catch a behaviour change and they read like
      coverage. Either cross-check each against the structural twin (as the error-matrix
      and POST-for-read halves now are) or say in each doc comment that it is a
      requirement pin only. Test: whichever is chosen.
      Done: cross-checked where there is something to cross-check against, and stated
      as a requirement pin where there is not. `the_two_global_rules_…` now also walks
      the document it is the preamble to — no component schema advertising a JSON number,
      every request-body schema denying unknown fields — so the promise is kept, not just
      typed. `the_put_outcome_rule_…` reads the two single-status exceptions out of
      `entities::PUT_ROUTES` and checks each is described with its status, so a third one
      fails until the prose says so. `the_list_filtering_contract_…` reads
      `entities::LIST_ROUTES` both ways: every list that takes a filter is named in the
      filtering paragraph, every list that takes none is not, and every filter name is
      spelled `?name=` there (the /attachments owner ids, described collectively, are the
      one classified exception). `the_list_reading_contract_…`'s page-size claim is now
      formatted from `row_history::DEFAULT_BROWSE_LIMIT`/`MAX_BROWSE_LIMIT`, so raising the
      cap cannot leave the prose behind; its **ordering** clause stays a requirement pin,
      said so in the doc comment, which names the five per-surface tests that pin the
      behaviour — deriving that set would mean reflecting over every list's `ORDER_BY`
      including the hand-written queries' SQL, which no scan can do honestly.
- [ ] Bound the unauthenticated Argon2 work on `POST /login`. The lockout now counts an
      attempt at the gate, so a source gets 5 verifies per cooldown — but
      `verify_password` still runs on the async handler with no `spawn_blocking`, and
      `Argon2::default()` is m=19 MiB, so concurrent first-time attempts from *many*
      sources are still unbounded transient memory and blocked tokio workers. Not
      urgent for a single-user deployment behind a proxy; it is the residual behind
      `docs/API.md`'s and README's "cannot be brute-forced online", which is also worth
      softening — an IPv6 /48 allocation rotates /64s freely, and the 4096-entry table
      evicts. Test: a threaded case asserting the concurrent bound (the current
      `the_gate_counts_each_attempt_rather_than_checking_then_acting` is sequential — it
      pins the right invariant, but its message claims more than it drives).
- [ ] Decide whether an **opt-in** `trusted_proxy` / `X-Forwarded-For` setting is wanted.
      Behind the documented nginx deployment every client shares one bucket, so 6 bad
      logins every 5 minutes denies the owner the sign-in page indefinitely (the bearer
      token is the documented escape). "Nothing trusts a forwarded header" is the right
      default; trusting one *only* when the operator declares the proxy is not the
      forgeable design the docs reject. Test: per-source isolation through a declared
      proxy, and that the header is ignored when none is declared.
- [ ] Add `panic_response` to `infra::http`'s `error_cases` table. It is named in the
      Error-body matrix as a 500 shape but is absent from the one sample table the docs
      and the OpenAPI description derive from; it returns a `Response` directly, so it
      is trivially includable. Test: the existing media-type/shape tests covering it
      like every other case.
- [ ] Resolve `/portfolio/open-parcels`' as-of default in one place. The handler resolves
      `as_of_or_today` and passes `Some(as_of)`; `domain::open_parcels::load` resolves
      `None` the same way, so the default is stated in the handler, the loader and the
      docs, and a change to the loader is masked by the handler. Test: the existing
      boundary tests, with the duplicate branch gone.
- [ ] Two stale references, both low: `REQUIREMENTS.md:1361-1362` still specifies
      `GET /reports/tax-report/years` and `POST /reports/tax-report`, which have answered
      405 since `ea21d55` (defensible — that file is the historical requirement text —
      but it is the one live document naming an endpoint that does not exist); and
      `docs/API.md`'s Server-side pagination limitation dates the filters 2026-09-24
      against a 2026-09-25 commit. Test: `doc_checks` for whichever wording lands.
