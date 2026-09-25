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
the [DONE.md](DONE.md) index. The open work is the 2026-09-24 REST API audit, in the two sections
below.

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
- [ ] Document list ordering, the POST-for-read set, and pagination as a first-class contract. API
      list order is ascending id/date (not the UI's newest-first), 12 read reports are POST+body,
      and `/reports/row_history` is the only cursor-paginated endpoint (and is shape-polymorphic:
      array vs `{entries, page_size, next_before_id}`). Add one summary table to `docs/API.md` so a
      client can learn these once. Test: `doc_checks` asserts the table (or the per-endpoint
      statements) exists.

## REST API audit — API improvements (2026-09-24)

Improvements that change behaviour or add features, called out by the same audit. These go beyond
fixing drift: they make the API safer and cheaper to build against, for the web UI and machine
clients alike. Each is closed by the tests named.

- [ ] Non-clobbering writes (B3). `PUT /collection/:id` silently replaces an existing row with no
      body and no created-vs-replaced signal, and there is no version/`If-Match`, so a stale or
      mistaken write clobbers a record with no confirmation. Make the outcome explicit and/or add
      optimistic concurrency: answer `201`+row on create and `204` (or a body) on update, or add a
      version/ETag so a stale read cannot overwrite a newer row. Test: round-trip tests assert the
      create-vs-update signal; an `If-Match`-stale write answers `412`/`409`.
- [ ] Server-side filtering (and paging) on the workhorse lists (B4). Only `closing_prices`,
      `attachments`, and `report_snapshots` accept query filters; `GET /trades`, `/income`,
      `/listings`, `/amma_statements`, … return the whole table, so a client fetches and filters
      entire tables. Add the obvious filters (`?listing_id=`, `?from=`/`?to=`,
      `?holding_account_id=`) and/or cursor paging to the entity lists. Test: API tests assert each
      filter narrows the result and unknown params still `422`.
- [ ] Make the as-at default explicit (B8). Omitting the as-of date silently means "today's live
      position" (`as_of_or_today`), never the open-ended sentinel, and `/portfolio/overview` has no
      as-of parameter at all while `performance`/`unrealised-gains`/`parcel-optimiser` each default
      their own. Expose `as_of_date` uniformly on the valuation reports (overview included) and
      state the default in `docs/API.md`. Test: API tests assert the omitted-date default and the
      as-of behaviour; `doc_checks` pins the stated default.
- [ ] Filter errored rows out of `GET /closing_prices` (B7). The list returns errored rows
      (`status:"error"`, `price:null`) interleaved with ok ones, so a client computing a valuation
      must filter client-side. Add a `?status=ok|error` filter (or `include_errored=false` default)
      so clean prices are one call. Test: API tests assert the filter.
- [ ] Rate-limit / lock out `POST /login` (B6). There is deliberately no lockout or rate limit
      (`infra/auth.rs:43-44`), so a single-credential deployment can be brute-forced. Add a bounded
      per-source lockout (or rate limit) — this reopens the prior scope decision, now that the API
      is also a script/LLM surface. Test: a lockout test over a small attempt budget.
