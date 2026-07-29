# TODO

Items are only marked done when a passing test exists for them.

Completed and decided (out-of-scope / not-reproducible) sections are archived in the topical `DONE/*.md` files, indexed by [DONE.md](DONE.md), to keep this list focused on active work. When a section here is fully done, move it into the matching `DONE/*.md` file rather than leaving it — see CLAUDE.md.

The sections below come from the **2026-07-13 improvement review** (a whole-project pass for
operational and test-strategy gaps, as distinct from the 2026-07-12 programming/domain review whose
findings are all closed in DONE.md), except where a section's heading names another source
(e.g. REQUIREMENTS). Each section records one finding; sections land in DONE.md as they are fixed
or decided.

## Authentication if ever exposed beyond localhost (honourable mention)
Currently handled correctly: the server binds `127.0.0.1` by default and exposing it is an explicit
`--host` opt-in documented as unauthenticated. Only matters if the server is ever exposed.
- [ ] If/when exposure is wanted: add an auth layer (e.g. a bearer token/basic-auth middleware over the whole router) before recommending `--host 0.0.0.0` for anything but a trusted LAN; until then this section records the decision that localhost-only is the accepted posture

## Open-parcel assembly duplicated across six reports (2026-07-29 Rust review)

`domain::cost_base` owns the per-parcel pipeline, but the *assembly* wrapped around it is
copy-pasted. The same ~70-line block — load Buy/DRP `ParcelRow`s, load `parcel_allocations` joined
to each sale's date, fold them into `qty_sold: HashMap<i64, Vec<(NaiveDate, Decimal)>>`, load AMIT
reductions + ROC events + split events + `FxRates`, then loop `sold_in_acquired_units` →
`remaining` → `adjusted_cost_base` → `into_aud_with` → `split_adjusted_quantity` — appears
essentially verbatim in `reports/portfolio.rs:97` (`db_holdings_on`),
`reports/unrealised_gains.rs:73` (`db_unrealised_gains`), `reports/open_parcels.rs:71`
(`db_open_parcels_on`) and `reports/performance.rs:387`, with partial repeats in
`reports/tax_report.rs:302`, `reports/realised_gains.rs:322`, and `reports/net_capital_gain.rs:309`.

This is the same class of finding as the 2026-06-10 "extract a shared adjusted-cost-base module"
item (DONE/reviews.md) one level up the call stack: that one unified steps 1–5, this one unifies
the loader around them. Today a fix to the split/ROC re-basing interaction has to land in six
places, and the copies have already drifted in ways that are correct but easy to get wrong when
edited (`up_to: Some(as_of)` vs `None`, `db_cost_base_reductions_up_to` vs
`db_cost_base_reductions`, quantity reported in as-of units vs current units).

The variation between the copies is small and parameterisable: an `as_of` cutoff (or `None`),
whether a joined `ticker` column is wanted, and whether the caller needs the full `CostBase`
breakdown or only `.adjusted`.

- [ ] Add `src/domain/open_parcels.rs` with a `load(conn, as_of) -> Result<Vec<OpenParcel>, sqlx::Error>` taking the caller's own `&mut SqliteConnection` (so it composes into each report's existing single-snapshot read transaction, per the house rule) and returning per-parcel `ParcelRow` + `remaining_as_acquired` + `remaining_as_of` + the AUD `CostBase` breakdown. Parcels fully consumed (`remaining <= 0`) are filtered out, as every copy does today
- [ ] Rewire `portfolio::db_holdings_on`, `unrealised_gains::db_unrealised_gains`, `open_parcels::db_open_parcels_on`, and `performance.rs:387` onto it; each keeps only its own aggregation/shaping. `open_parcels` needs its joined `ticker` — resolve it as a separate lookup rather than pushing a join option into the shared loader
- [ ] Assess `tax_report.rs:302`, `realised_gains.rs:322`, and `net_capital_gain.rs:309` separately: these walk *sold* parcels, not open ones, and may only share the reference-data loading (ROC/split/AMIT/FX). Either extract that narrower piece or record here why they stay as they are
- [ ] Tests: the `ato_examples.rs` suite is the safety net (as it was for the cost-base extraction). Add a `domain::open_parcels` unit test per behaviour the copies encode — as-of cutoff excludes later trades/sales, split re-basing of an allocated quantity, AMIT/ROC reduction applied, fully-consumed parcel filtered out — plus an assertion that portfolio/unrealised/open-parcels agree on total cost base for the same fixture (the identity the duplication currently risks)

## Decimal columns bypass the sqlx type system (2026-07-29 Rust review)

Every TEXT-stored decimal is read through a hand-written `FromRow` and written through
`.bind(x.to_string())`: 19 hand-written `impl sqlx::FromRow` blocks across 16 files, ~100
`row_dec`/`row_opt_dec` calls, and 109 `.bind(<decimal>.to_string())` sites. CLAUDE.md's
"never `.parse().unwrap_or(Decimal::ZERO)`" rule and "new monetary columns are TEXT" rule are
therefore enforced by review discipline on every new column, not by the compiler.

sqlx 0.9 can carry this itself. A local newtype implementing `Type`/`Decode`/`Encode` for Sqlite,
plus `#[sqlx(try_from = "…")]` on the field, lets row structs go back to `#[derive(sqlx::FromRow)]`
with plain `Decimal`/`Option<Decimal>` fields:

```rust
// infra/decimal.rs
pub struct Money(pub Decimal);             // TEXT-backed Type + Decode + Encode
pub struct OptMoney(pub Option<Decimal>);  // decodes SQL NULL itself
impl From<Money> for Decimal { … }
impl From<OptMoney> for Option<Decimal> { … }

#[derive(Serialize, Deserialize, sqlx::FromRow)]
pub struct InterestIncome {
    pub id: i64,
    #[sqlx(try_from = "Money")]    pub amount: Decimal,
    #[sqlx(try_from = "OptMoney")] pub gross_amount: Option<Decimal>,
}
```

Prototyped and runtime-verified against sqlx 0.9 before writing this section: `123.4567890123`
round-trips exactly, `NULL` decodes to `None`, and a malformed value fails with
`error occurred while decoding column "amount": invalid decimal "oops"` — the column name now comes
from sqlx rather than a hand-passed string literal that can drift from the actual column, so
diagnostics are strictly better than `parse_dec`'s. Two things worth knowing up front: sqlx 0.9's
`Encode::encode_by_ref` takes `&mut SqliteArgumentsBuffer` (not 0.8's
`&mut Vec<SqliteArgumentValue>`), and `impl From<OptMoney> for Option<Decimal>` is orphan-legal
because the local type sits in argument position.

- [ ] Add `Money`/`OptMoney` to `infra/decimal.rs` with `Type`/`Decode`/`Encode` for Sqlite and the `From` conversions; keep `parse_dec` for the non-`FromRow` callers that read a scalar out of an ad-hoc query
- [ ] Convert the 19 hand-written `FromRow` impls to derives, entity by entity (each file is independently convertible, so this can land as several small commits): `entities/{amit_adjustment,amma,cgt_settings,closing_price,ess_statement,income,inheritance,interest_income,investment_expense,parcel_allocation,rba_fx_rate}.rs`, `entities/{corporate_action/model,trade/model}.rs`, `reports/{performance,realised_gains}.rs`, `domain/cost_base.rs`
- [ ] Replace the 109 `.bind(x.to_string())` sites with `.bind(Money(x))` so writes go through the same type as reads
- [ ] Tests: a `Money`/`OptMoney` round-trip test in `infra::decimal` pinning full precision preserved, `NULL` → `None`, and a malformed value producing a decode error naming the column (the behaviour `db_malformed_decimal_is_an_error_not_zero` pins today, now at the type level). The existing per-entity CRUD tests cover the conversions; a green suite with the `FromRow` impls gone is the gate
- [ ] Once converted, consider whether `db::tests::migrations_store_decimals_as_text_never_real` can be strengthened to also assert every monetary column's Rust field goes through `Money`/`OptMoney` — or record here that the derive makes it unnecessary

## Entity CRUD scaffolding duplicated, and the DELETE 404 contract has drifted (2026-07-29 Rust review)

19 entity modules contain a byte-identical `async fn list`, and `get_one`/`delete` are identical
modulo the type and the message. The duplication is cheap on its own, but it has already let the
404 contract drift three ways across the delete handlers:

| style | count | user-visible effect |
| --- | --- | --- |
| `StatusCode::NOT_FOUND` | 8 — `amma`, `amit_adjustment`, `cgt_settings`, `exchange`, `exchange_holiday`, `drp_enrolment`, `attachment`, `corporate_action/http` | empty body; the web UI shows a bare "HTTP 404" |
| `Err(ApiError::NotFound)` | 1 — `listing` | empty body, same effect |
| `ApiError::not_found("no X with that id")` | 9 — `sell`, `trade/http`, `transfer`, `income`, `inheritance`, `holding_account`, `ess_statement`, `interest_income`, `investment_expense` | UI toast names what was missing |

That is exactly the split `ApiError::NotFoundWithReason` was introduced to remove
(DONE/reviews.md, "Stop swallowing errors behind bare 500s"): operation endpoints name the missing
prerequisite, and a DELETE is an operation endpoint. The fix is worth doing for the contract alone;
the boilerplate removal is the bonus.

- [ ] Make the DELETE 404 contract uniform: every entity delete returns `ApiError::not_found("no <noun> with that id")`. Smallest form is one shared `infra::http::deleted(found: bool, noun: &str) -> Result<StatusCode, ApiError>` helper — do this first and independently, since it is the user-visible half
- [ ] Then fold the mechanical scaffolding: a `CrudEntity` trait (`const TABLE`, `const COLUMNS`, `const NOUN`, plus the model type) with generic `list_handler`/`get_handler`/`delete_handler` in `infra/http.rs`. `db_upsert` stays per-entity — that is where the write-time invariants live and it must not be generated away
- [ ] Update `docs/API.md`'s Response codes section to state that a DELETE of a missing row returns 404 *with* a plain-text reason, matching the other operation endpoints
- [ ] Tests: one test per converted entity asserting `DELETE /<entity>/{unknown-id}` is 404 with a non-empty body naming the noun (the 8 bare-404 entities have no such assertion today); the existing per-entity list/get tests cover the generic handlers

## 37 error enums with hand-written `From<sqlx::Error>` (2026-07-29 Rust review)

There are 37 error enums and 32 hand-written `impl From<sqlx::Error>` blocks that all read
`fn from(e: sqlx::Error) -> Self { X::Db(e) }`. None of them implement `Display` or
`std::error::Error`, so when one is wrapped into `ApiError::Internal` the log message quality
depends entirely on whatever the `From<EntityError> for ApiError` arm writes, and the underlying
error's `source()` chain is lost.

`thiserror` gives `#[from]`, `Display`, and `source()` chaining for free. It is a proc-macro-only
dependency (no runtime surface) with a clean advisory history, so it passes the `cargo deny check
advisories` gate.

- [ ] Add `thiserror` and convert the 37 enums: `#[derive(thiserror::Error, Debug)]` with `#[error("…")]` per variant and `#[from]` on the `Db(sqlx::Error)` variant, deleting the 32 `From<sqlx::Error>` impls
- [ ] Keep every `impl From<EntityError> for ApiError` exactly as it is — those carry the user-facing 422 wording that `docs/API.md` documents and must not become derived `Display` output
- [ ] Tests: the existing per-entity rejection tests already assert the 422 bodies, so a green suite is the gate that the user-facing messages are unchanged. Add one test asserting a wrapped `sqlx::Error` still reaches the log through `ApiError::Internal` with its own message intact (extending `infra::http::tests::internal_logs_the_wrapped_error_at_error_level`)

## Over-long functions (2026-07-29 Rust review)

23 functions exceed 100 lines in the non-test build (`cargo clippy -- -W clippy::too_many_lines`).
The tail is what matters:

| lines | function |
| --- | --- |
| 362 | `reports/activity.rs:128` |
| 327 | `reports/tax_report.rs:693` |
| 249 | `entities/transfer.rs:208` |
| 249 | `entities/demerger.rs:130` |
| 234 | `entities/scrip_exchange.rs:126` |
| 212 | `reports/tax_summary.rs:342` |
| 206 | `reports/realised_gains.rs:358` |

The three entity ones are all the same shape — validate → walk parcels → build replacement rows →
write, in one transaction — and split naturally along those seams. The open-parcel and `Money`
sections above will already shrink several of these, so this is deliberately sequenced last.

- [ ] Split `reports/activity.rs:128` (362 lines) — the largest single function in the codebase; treat as its own task
- [ ] Split `reports/tax_report.rs:693` (327 lines)
- [ ] Split the three rollover/transfer operations (`transfer.rs:208`, `demerger.rs:130`, `scrip_exchange.rs:126`) along the shared validate → walk → build → write seam, ideally sharing the extracted pieces rather than each growing its own
- [ ] Re-measure after the open-parcel and `Money` refactors land and record here which of the remaining sub-250-line entries are actually worth splitting — a long function that is one flat, well-commented sequence is not automatically a defect
- [ ] Tests: pure refactor, so the gate is the existing suite plus `ato_examples.rs` passing unchanged; no behaviour change means no new test, which is the one case where an item here closes without one

## HTTP test boilerplate (2026-07-29 Rust review)

`test_support.rs` solved the *data* half of test setup (builders for the wide structs) but not the
HTTP half: 274 `Request::builder()` calls across 55 files and 130 copies of
`.collect().await.unwrap().to_bytes()`. Only `entities/closing_price.rs` has local `post_json`
/`put_json` helpers (`:2314`, `:2424`); every other module open-codes the request and the body
decode. Since tests are ~60% of the tree (41.6k of 69.8k lines), this is where line-count reduction
is largest — but it is lower value than the sections above, so it should not jump the queue.

- [ ] Add the HTTP half to `test_support.rs`: an `ApiClient` wrapping `app::router(pool, registry, fetcher)` (or the narrower `router().with_state(pool)` where a test doesn't need the registry/fetcher) with `get_json::<T>(path)`, `put_json(path, &body) -> StatusCode`, `post_json::<T>(path, &body)`, and a `status_and_body(path)` for the rejection tests that assert on the 422 text
- [ ] Migrate test modules onto it opportunistically — when a module is already being touched for one of the sections above, rather than as one large mechanical commit
- [ ] Tests: the migrated tests are the test; the gate is the full suite passing unchanged after each module's migration

