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
