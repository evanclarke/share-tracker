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
and archived in [`DONE/api.md`](DONE/api.md), and so is the 2026-10-04 agent-usability pass that
followed it (OpenAPI bodies, ids, descriptions and slices, `?dry_run=true`, the live-API skill,
trustworthy `required` lists, and reasoned GET-one `404`s). The Budget section below came from the
2026-10-08 sweep of the ATO mirrors (`docs/ato/OVERVIEW.md`, *Full sweep, 2026-10-08*). The findings
of the 2026-10-10 metric-scored code review (financial correctness 8, error handling 8.5,
concurrency/integrity/performance 8, security 8.5, architecture 6, tests 8, hygiene 9 — out of 10)
are all closed and archived in [`DONE/reviews.md`](DONE/reviews.md), the last of them the
`proptest` item of its test-suite section (2026-10-11), whose properties found and fixed three
defects of their own.

The 2026-10-11 sweep for invariants stated only in comments (moving each into a type) is closed and
archived in [`DONE/reviews.md`](DONE/reviews.md), and so is the re-basing exactness section found
verifying it (closed 2026-10-11: `AcquiredUnits` is now an exact fraction).

## 2026–27 Budget CGT changes — announced, not law (ATO mirror sweep 2026-10-08)
The ATO's CGT discount, cost base, how-to-calculate and indexation pages have carried a banner
since 29 June 2026: "Recent changes to capital gains tax (CGT) announced in the 2026–27 Federal
Budget don't apply to Tax Time 2026", with "Resources will be available at a later date". As
reported (not yet ATO guidance), the measure replaces the 50% discount with **cost-base indexation
plus a 30% minimum tax** on real gains for assets held 12 months or more, from **1 July 2027**,
with an asset bought before and sold after that date taxed under the existing rules for the gain
accrued to 1 July 2027 and the new rules after it. Nothing is modelled or mirrored yet: there is
no legislation or ATO guidance to implement against, and every disposal this system can hold today
falls under the existing rules.
- [ ] Watch: when the measure is legislated and the ATO publishes guidance, mirror it into
  `docs/ato/` and index it in `docs/ato/OVERVIEW.md` (re-fetch the four bannered pages then too)
- [ ] NEEDS DECISION (owner), once the rules are known: whether and how to model the 1 July 2027
  split. Known shape of the inputs: a per-parcel **market value at 1 July 2027** to divide the gain
  (the closing-price table is the natural source); a CPI series that runs past September 1999
  (`cpi_quarters` is frozen at the 1999 freeze today); and the 30% minimum tax, which turns on the
  taxpayer's marginal rate — outside the data model, like the FITO offset limit
- [ ] Whatever is decided, make sure a closing price is stored for every held listing on the last
  trading day before 1 July 2027 (the scheduled price import plus `closing_price`'s held-days
  backfill), since a missing one cannot be fetched reliably years later

## Google Rust style guide pass (2026-10-23) — correctness-adjacent findings
A pass over the non-test code against <https://google.github.io/styleguide/rustguide.html>, measured
by clippy with ~35 extra guide-mapped lints plus greps for what clippy cannot see. This is
application code (one binary crate), so the guide's API-design rules are strong recommendations
rather than binding, and its Chromium-only rules (flat layout, small crates, no `async`, gtest) do
not apply. Deliberately **not** taken up: the six `macro_rules!` (each exists because a utoipa
annotation cannot describe a generic fn), the `Arc`s (shared across spawned tasks), and the 40
functions over 100 lines (the 2026-08-28 complexity audit decided flat guard chains stay). Where an
item below is closed by enabling a lint, the CI clippy gate (`-D warnings`) is its test.
- [x] Unsound test: `infra/logging.rs:32` calls `unsafe { std::env::remove_var("RUST_LOG") }` with
  no `SAFETY:` comment, and none could honestly be written — edition 2024 made it unsafe because
  mutating the environment while other threads run is unsound, and `cargo test` is multithreaded.
  Make `build_filter` take the `RUST_LOG` value as a parameter and test that pure function, leaving
  no `unsafe` outside `test_support`'s documented FFI. Done 2026-10-23: `build_filter(Option<&str>)`
  is tested as a pure function, and `logging::tests::unsafe_code_is_confined_to_test_support`
  scans every file's code for the `unsafe` keyword
- [x] Wildcard arms over first-party enums, where a new variant would silently take the catch-all
  instead of failing to compile — list the variants: `ActionKind` (`buyback_participation.rs:217`,
  `rights_exercise.rs:377`, `rights_sale.rs:608`), `ActionType` (`corporate_action/model.rs:850`),
  `ParcelOrigin` (`trade/new_parcel.rs:65`), `FxError` (`infra/fx.rs:200`), `StoreError`
  (`distribution_event.rs:332`). The `SellError` pass-through `other =>` arms in the six operation
  files may stay, each with a comment saying why; third-party enums (yfinance, `serde_json::Value`,
  `Weekday`, sqlx `ErrorKind`) are out of scope. Test: enable `clippy::wildcard_enum_match_arm` (or
  a scan) with the deliberate exceptions `#[allow]`ed and reasoned
  Done 2026-10-23: `Cargo.toml` now has a `[lints.clippy]` table enabling
  `wildcard_enum_match_arm` for production code (`main.rs` exempts `cfg(test)`). Every first-party
  site lists its variants, and so do the closed third-party enums (`Weekday`, `serde_json::Value`).
  The SellError pass-throughs and the `#[non_exhaustive]` third-party matches (yfinance `YfError`/
  `Action`, sqlx `ErrorKind`) carry a reasoned fn-level `#[expect]`, because an arm-level attribute
  is not honoured. Gate mutation-checked: a restored `_` in `new_parcel.rs` fails clippy
- [x] Named fields over tuples, worst first where the pair is same-typed and swappable:
  `email::Document::render -> (String, String)` (HTML vs plain text), `decimal::apportion ->
  (Decimal, Decimal)` (share, rest), `adjustments::split_ratio`/`rebase_ratio -> (Decimal, Decimal)`
  (new, old) and `Ratio::parts`, `http::Upserted<E> = (Upsert, Option<E>)`, the
  `(id, Upsert, Option<T>)` returns of `listing.rs:458`/`amma.rs:407`/`drp_enrolment.rs:247`, and
  `fx::Provisional::into_parts -> (T, bool)` (also a bool-as-enum case)
  Done 2026-10-23: `email::Rendered { text, html }`, `decimal::Apportioned { share, rest }`,
  `adjustments::SplitRatio { new, old }` (from `split_ratio`, `rebase_ratio` and also
  `price_basis_ratio`, which had the same `(new, old)` shape) and `Fraction { num, den }` (from
  `AcquiredUnits::parts`; `Ratio::parts` was a misnaming of it), `http::Upserted { outcome,
  created }`, and `fx::Parts { value, provisional }`. The twelve private `write` fns returned
  `(id, Upsert, Option<T>)`, but every caller discarded the id, so they now return `Upserted<T>`
  instead. The compiler enforces the shapes; the existing tests for each (the `apportion`
  property, the `split_ratio` interval tests, every upsert round-trip, the fx fallback test)
  now assert against the named fields
- [x] `bool` parameters whose meaning is invisible at the call site (19 fns): `Written::new(dry_run:
  bool, …)` (`infra/respond.rs:160`) should take the existing `WriteMode`; the auth flag threaded as
  `bool` through `web::router`, `web::index_html`, `app::api_routes` and `RequestGuard::new` should
  be one `AuthMode` enum; review the rest (`http::deleted(found: bool, …)`, `Upsert::of(existed:
  bool)`) and either convert or name the argument at each call site
  Done 2026-10-23: `Written::new` takes a `WriteMode` (plus `Written::upserted(Upsert, …)` for
  `POST /closing_prices/fetch`'s create-or-replace), and the AMMA generation's `preview` is a
  `WriteMode` through `write_or_preview` too. `infra::auth::AuthMode { Required, Open }` replaces
  the auth flag in those four and in `api_spec`'s `document_for`/`publish`/`components`/`router`.
  Also converted: `crud_upserted` takes the `Upsert` (so `amit_adjustment`'s `outcome ==
  Upsert::Replaced` round trip is gone), `Auth::new`'s `secure_cookie` is `CookieSecurity`,
  `login_submit`'s `html` is `LoginClient`, `resolve_live_prices`/`price_holdings` take
  `LiveQuotes`, the scheduler's `db_finish_run`/`db_record_run` take the `&JobOutcome` in place of
  `success` + `error` + `note`, `units_held_problem` takes `AdjustmentSet`, and `Occupants::new`'s
  two same-typed flags became a struct literal. `deleted(found, …)`'s three callers now bind
  `found` first. Twelve `bool` parameters stay, each named at its call site (`Upsert::of` and
  `LiveQuotes::requested` are the two bool-to-enum adapters). Test:
  `app::tests::no_function_takes_an_unclassified_bool_parameter` scans every non-test file and
  fails on a `bool` parameter not in its reasoned `BOOL_PARAMETERS_ALLOWED` (and on a stale entry)
- [x] `#[must_use]` is on one item in the tree (`fx::Provisional`); clippy's `must_use_candidate`
  never fires in a binary crate. Add it to the pure domain arithmetic whose dropped result is a
  silent money error — `mul_div`, `apportion`, `split_ratio`, `CostBase::into_aud_with`,
  `UnitsAt::rebased` — and to `Self`-returning builders (`ApiClient::with_header`). Test: a scan or
  compile-fail-free check that each named fn carries it
  Done 2026-10-23: the result *types* carry it where they have one — `Apportioned`, `SplitRatio`,
  `AcquiredUnits`, `Fraction`, `UnitsAt`, `CostBase<C>`, `Aud` — which covers `apportion`,
  `split_ratio`/`rebase_ratio`/`price_basis_ratio`, `rebased`, `into_aud_with`,
  `adjusted_cost_base` and the unit conversions; the `Decimal`-returning arithmetic carries it on
  the fn (`mul_div`, `to_cents`, `rebase`, `contemporaneous_price`, `reduction_for_units`, the AMIT
  `per_unit_for`, `prorated_initial_cost`, `reduction_for`), as do `ApiClient::with_header`/
  `with_peer` and the six `test_support` row-builder types. No existing call site dropped a result.
  Test: `infra::decimal::tests::money_arithmetic_and_builders_are_must_use` scans for the attribute
  on each listed type and fn, and checks each type-covered fn still answers its must-use type
- [ ] Implement `FromStr` instead of ad-hoc parse methods (none exist in `src` today):
  `WorthlessEvent::from_str` (`corporate_action/model.rs:35` — the guide's own
  `should_implement_trait` example), `RateMonth::parse` (`fx.rs:78`), `BasePath::parse`
  (`config.rs:342`), the email address `parse` (`email.rs:123`), `AuditedTable::parse`
  (`row_history.rs:105`); sqlx decode adapts the trait's error where it needs `sqlx::Error`
- [ ] Encode the rules: extend the `[lints.clippy]` table in `Cargo.toml` (added with
  `wildcard_enum_match_arm`) (so CI's `-D warnings` enforces
  it) with at least `unwrap_used` (+ `allow-unwrap-in-tests = true` in `clippy.toml`),
  `undocumented_unsafe_blocks`, `match_wildcard_for_single_variants`, `str_to_string`,
  `redundant_clone`, `needless_pass_by_value`, `format_push_string`, `cast_possible_truncation`.
  The guide's rustfmt options (`imports_granularity = "Module"`, `hex_literal_case`,
  `format_macro_matchers`) are nightly-only, so stable `cargo fmt --check` cannot gate them — record
  that decision in a `rustfmt.toml` comment rather than adopting them silently

## Google Rust style guide pass (2026-10-23) — mechanical clean-ups
Each closed by the lint named in the section above once it is enabled and clean.
- [ ] Bare `unwrap()` at `entities/exchange_holiday.rs:187-188` (1 Jan / 31 Dec construction) →
  `expect` stating why it cannot fail. `main.rs`'s `expect`s stay (developer-facing binary)
- [ ] Possibly-truncating `as` casts → `try_from`: `closing_price/fetcher.rs:342` (usize→i32),
  `tax_year_settings.rs:151` (i64→i32), `row_history.rs:372` (i64→usize)
- [ ] String building: 15 `s += &format!(…)` (`infra/email.rs` 7, `reports/activity.rs` 6,
  `api_spec.rs` 2) → `write!`; the `+` concatenation at `api_spec.rs:278`; and the chained
  `.replace("{{X}}", …)` templating in `web.rs:137` / `infra/auth.rs:1189` — at minimum a comment
  noting the order dependence (a substituted `{{ERROR}}` body containing `{{THEME}}` would be
  expanded by the next replace), or a single-pass substitution
- [ ] `Iterator::for_each` → `for` loops at `api_spec.rs:395`, `:1846`, `:1848`, `:2514`
- [ ] 22 `.to_string()` calls on `&str` → `to_owned()` (`clippy::str_to_string`, auto-fixable)
- [ ] Nine by-value arguments never consumed (e.g. `config.rs:232`, `decimal.rs:286` → `&str`;
  `http.rs:425`; `scheduler/registry.rs:215`; `amit_adjustment.rs:566`; `trade/checks.rs:330`) and
  the redundant clone at `scheduler/registry.rs:402`

## Google Rust style guide pass (2026-10-23) — imports, module layout and docs
- [ ] 656 inline fully-qualified paths where the guide wants types imported and used unqualified
  (hotspots: `api_spec.rs` 121, `infra/db.rs` 37, `scheduler/registry.rs` 20, `inheritance.rs` 19,
  `app.rs` 19; mostly `crate::infra::http::…`, `crate::domain::cost_base::…`,
  `crate::infra::fx::…`). Keep *functions* module-qualified (`fx::to_aud`); import types
- [ ] 84 `use` lines nesting a path inside braces (`use utoipa_axum::{router::OpenApiRouter,
  routes};`) → one module per `use` statement. Needs a scan test, since the rustfmt option that
  would enforce it is nightly-only
- [ ] Non-leaf modules carrying APIs beyond tolerated glue: move `reports/mod.rs`'s
  `TAXPAYER_BASIS`/`listing_label`/`account_label` into a leaf; slim `entities/mod.rs` (1353 lines)
  and `reports/health/mod.rs` to declarations, re-exports and the router; move the split parents'
  large inline test modules (`trade.rs`, `corporate_action.rs`, `scheduler.rs`) into a `tests`
  submodule as `closing_price` already did — code split and test move in separate commits, per
  CLAUDE.md
- [ ] Docs: `//!` module headers for the 22 files lacking one; the 160 `clippy::doc_markdown`
  missing-backtick findings (auto-fixable); and item docs across `domain/` (the financial lower
  layer). Blanket `missing_docs` (~2,100 items) is deliberately not adopted — the guide aims it at
  public library APIs
