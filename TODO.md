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
- [ ] Make `CrudListFilter`'s bind-not-interpolate rule structural. `apply_filter`
      takes a raw `QueryBuilder`, so `qb.push(format!(" AND ticker = '{v}'"))` compiles
      and passes every test; all ~14 filters go through `push_eq`/`push_date_range`
      today by convention only. Either scan `impl CrudListFilter` bodies for a `push(`
      whose argument is not a literal, or narrow the trait so a filter can only reach
      the helpers. This repo enforces its comparable rules structurally
      (`write_side_modules_never_begin_a_deferred_transaction`), which is the argument
      for doing it. Test: the scan, or the narrowed signature refusing to compile.
- [ ] Derive the OpenAPI route table's remaining hand-maintained fields. `(path, verb)`
      is pinned both ways, every PUT row's statuses are pinned twice, and the query
      parameters now come from each route's `Query<T>` type — but ~110 rows' success
      statuses, request/response schema names and summaries are still typed by hand.
      The proof this drifts is in the archive: `59bc2e6` had to hand-edit 536 lines when
      the PUT statuses changed, and nothing would have failed had it not. Deriving them
      needs handler metadata the tree does not carry (a per-handler attribute, or
      `utoipa::path` on every handler), so this is a design decision, not a fix.
      Test: a both-ways comparison per field, as `every_served_route_is_documented_and_nothing_else_is`
      already does for `(path, verb)`.
- [ ] Decide the four `api_spec` pins that assert `DESCRIPTION` against `DESCRIPTION`
      (`the_two_global_rules_…`, the ordering/pagination half of
      `the_list_reading_contract_…`, `the_put_outcome_rule_…`,
      `the_list_filtering_contract_…`). Each checks that a documentation *requirement*
      is met, and the facts they describe are pinned behaviourally elsewhere — so they
      are not worthless, but they cannot catch a behaviour change and they read like
      coverage. Either cross-check each against the structural twin (as the error-matrix
      and POST-for-read halves now are) or say in each doc comment that it is a
      requirement pin only. Test: whichever is chosen.
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
