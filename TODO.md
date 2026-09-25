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
the [DONE.md](DONE.md) index. The open work is the 2026-09-24 REST API audit, in the three sections
below.

## REST API audit — documentation fixes (2026-09-24)

An audit of the HTTP surface against `docs/API.md`. The code is accurate; every item here is a
`docs/API.md` correction, each closed by a `doc_checks.rs` assertion that the required text is now
present/absent.

- [ ] Document `date_received` as a **required** AMMA-statement field. `src/entities/amma.rs:109`
      declares `pub date_received: NaiveDate` with no `#[serde(default)]`, so it is mandatory — yet
      `date_received` appears nowhere in `docs/API.md`. Add it to the AMMA statements section
      (docs/API.md ~531–572) as required. A machine client omitting it gets an unexplained
      `422 missing field date_received`. Test: `doc_checks` asserts the AMMA section names
      `date_received` as required.
- [ ] Complete the `201 Created` enumeration in the Response codes table (docs/API.md:1800). It
      lists the operation endpoints but omits the 14 standard `POST /<collection>` creates and
      `POST /ess_statements/:id/vest` — all `201` in code — contradicting the table's own "Creating
      a record" section (docs/API.md:77). Test: `doc_checks` asserts the `201` row names the
      collection-POST create path and `vest`.
- [ ] Document the `preference` listing field. `src/entities/listing.rs:116`
      (`#[serde(default)] pub preference: bool`, the 90-day preference-share flag read by franking
      at-risk) is never named in the Listings section (docs/API.md:123–191). Test: `doc_checks`
      asserts the Listings section documents `preference`.
- [ ] Drop the stale `409` from the "Error bodies" list (docs/API.md:1836). No code path returns
      HTTP 409 (no `StatusCode::CONFLICT`; `ApiError` has no 409 variant), and the Response codes
      table itself has no 409 row, so the doc is self-inconsistent. Test: `doc_checks` asserts the
      error-bodies list names only the codes the code actually returns.
- [ ] Document the three DRP residual columns `PUT /trades` accepts —
      `src/entities/trade/model.rs:327-332` (`residual_brought_forward`/`residual_carried_forward`/
      `residual_paid_out`, all `#[serde(default)]`) — which are referenced only indirectly
      (docs/API.md:1848), never as trade body fields. Note they are the reinvest-created DRP's
      server-managed residual chain, not client-writable in practice. Test: `doc_checks` asserts the
      Trades section names them (or says they are server-managed).

## REST API audit — consistency fixes (2026-09-24)

- [ ] Fix the natural-key DELETE 404 wording. `infra::http::deleted` hard-codes
      `no {noun} with that id` (`src/infra/http.rs:156`), which is wrong for `DELETE /exchanges/{mic}`
      (key is `mic`, `src/entities/exchange.rs:123-124`) and `DELETE /tax_year_settings/{tax_year}`
      (`src/entities/tax_year_settings.rs`). Give each a key-specific body (`no exchange with that
      mic`, `no tax year settings row for that year`); `exchange_holidays` already hand-writes its
      composite-key message (`exchange_holiday.rs:252-254`). Test: extend
      `entities::tests::deleting_a_missing_row_is_404_naming_what_was_missing` to assert the
      key-specific wording.
- [ ] Make `GET /rights_sales/{id}` consistent with the empty-body GET-one 404.
      `src/entities/rights_sale.rs:737` returns `no rights sale with that id`, the only GET-one that
      carries a body; every other GET-one uses the empty `ApiError::NotFound`. Return the empty
      body, or document the divergence as deliberate. Test: assert the 404 body is empty (or, if
      kept, pin the wording).
- [ ] Resolve the report path namespace/case split. `/portfolio/*` is kebab-case and `/reports/*`
      is snake_case, but `/reports/tax-report` (kebab) sits beside `/reports/rollover_consistency`
      (snake), and route names diverge from module names (`mic_validation` →
      `/reports/exchange_mic_validation`). Pick one scheme for the whole report surface and either
      normalise the paths (UI `REPORTS` config and docs following) or state the rule in
      `docs/API.md`. Test: `doc_checks` asserts the chosen rule is stated (and any path change
      keeps the UI config and route table in agreement).

## REST API audit — LLM / machine-client surface (2026-09-24)

The API is also consumed by LLMs and scripts, not just the web UI. These close the gaps the audit
found for a non-browser client; the first is the largest.

- [ ] Emit a machine-readable API description (OpenAPI or JSON-Schema). The whole contract is
      currently the ~616 KB prose `docs/API.md` (1,936 lines), with the critical global rules
      (money/quantity as JSON strings, `deny_unknown_fields` on every body) stated only in prose at
      the end (docs/API.md:1838–1865). Generate it from the route table + serde structs and pin it
      with a test (like the existing `doc_checks`) so it cannot drift. Test: a check that the
      generated spec covers every route and carries the string-decimal and deny-unknown-fields
      rules.
- [ ] Pin outbound money/quantity serialization. Responses serialize `Decimal` as strings only by
      accident of `rust_decimal`'s default (`Cargo.toml:16` `features=["maths"]`; no
      `serialize_with` anywhere in `src`). A dependency-feature change would silently turn every
      money/quantity field into a float. Add an explicit string codec (the write-side mirror of
      `infra::decimal::strict_decimal`) and a test that a money field serializes as a JSON string.
- [ ] Standardise error responses for machine clients. Every error body is `text/plain` with a
      status/body matrix (422/400/413/502/503 carry text; GET 404 is empty; internal 500 is empty;
      job 500 carries text). Either adopt one JSON error envelope, or document the matrix as an
      explicit contract in `docs/API.md`. Test: `doc_checks` pins the chosen contract.
- [ ] Document list ordering, the POST-for-read set, and pagination as a first-class contract. API
      list order is ascending id/date (not the UI's newest-first), 12 read reports are POST+body,
      and `/reports/row_history` is the only cursor-paginated endpoint (and is shape-polymorphic:
      array vs `{entries, page_size, next_before_id}`). Add one summary table to `docs/API.md` so a
      client can learn these once. Test: `doc_checks` asserts the table (or the per-endpoint
      statements) exists.
