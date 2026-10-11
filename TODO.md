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

The type-system section at the end came from a 2026-10-11 sweep for invariants stated only in comments.

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

## Comment-stated invariants the type system should enforce (review 2026-10-11)
A sweep for properties that a doc comment (or a source-scan allowlist) states but nothing structural
holds. Each item moves the rule into a type, so a violation stops compiling rather than producing a
wrong figure. Ordered by risk: the first two guard against silently wrong tax figures in the cases
the suite is least likely to cover (a foreign-currency parcel; a split between purchase and sale).
- [x] **AUD vs native currency.** `CostBase::into_aud_with` (`src/domain/cost_base.rs`) maps
  `CostBase → CostBase`, so a native-currency breakdown can be aggregated with an AUD one or
  converted twice, and `at_rate`'s pass-through at rate 1 hides it from AUD fixtures. Give
  `CostBase` a currency marker (`CostBase<Native>` / `CostBase<Aud>`, conversion only via
  `into_aud_with`), then an `Aud(Decimal)` newtype for the inputs documented "expected in AUD"
  (`current_prices` in `reports::unrealised_gains`/`reports::performance`, and the like)
  — Done 2026-10-11: `CostBase<Native>`/`CostBase<Aud>` (private marker field, `into_aud_with` the
  only conversion; indexation and `OpenParcel` take `CostBase<Aud>`), and `infra::currency::Aud`
  carried by every report price map, `LiveValuation`/`ListingValuation::aud_price` and the
  request `prices` maps (`strict_aud_map`, OpenAPI schema unchanged). Made only by
  `fx::apply_rate_to_aud` or `Aud::stated`, whose production call sites are pinned by
  `infra::currency`'s `aud_is_stated_only_where_the_amount_is_known_to_be_aud`
- [ ] **Unit basis of a quantity.** As-acquired and as-of/sale-date quantities are both `Decimal`;
  `open_parcels::db_units_sold`'s doc says the sale-date quantity "must" be re-based before it is
  subtracted, and `OpenParcel::remaining_as_of` vs `parcel.quantity` is distinguished only in prose
  (~180 comments state a basis). Introduce distinct types (e.g. `AcquiredUnits` / `UnitsAt`) with
  `adjustments::split_adjusted_quantity`/`as_acquired_quantity`/`sold_in_acquired_units` as the
  only conversions, carried by `UnitsConsumed::quantity` and `OpenParcel::remaining_as_of` — the
  quantity counterpart of `cost_base::Held`
- [ ] **Valuation-only FX fallback.** `fx::ValuationRate` exposes `pub rate`, so a caller can drop
  `provisional` ("the caller must surface this"), and valuation-only use is held by the
  `VALUATION_RATE_CALLERS_ALLOWED` file-name scan. Make the rate private and offer only an `apply`
  returning the value with its provisional flag attached; once tax paths accept only plain/`Aud`
  amounts, retire the allowlist scan
- [ ] **`sale_trade_id: i64` overloaded.** `realised_gains::RealisedGainLoss::sale_trade_id` (and
  the tax report's `DisposalParcelRow` copy) is a trade id or a `rights_sales` id depending on the
  separate `source` field. Replace the pair with an enum (e.g. `DisposalId { Sell(TradeId),
  RightsSale(RightsSaleId) }`); the API has no external clients, so reshaping the JSON is allowed
  (update `docs/API.md`, and the CLAUDE.md note on the bare field)
- [ ] **Sorted-input preconditions.** `parcel_optimiser::allocate_strategy` says "`parcels` must
  already be FIFO-sorted" but is `pub` with four callers (one in `reports::net_capital_gain`): take
  the already-sorted `Candidates` (or a `FifoSorted` newtype) instead of a slice. Lower priority:
  the `adjustments` event loaders' "sorted by date (then id)" promise, which the ordered
  floor-at-nil walks rely on
- [ ] **`base_path` normalised form.** `config::normalise_base_path` produces `""` or `/a/b`, which
  ~15 functions across `web.rs`, `api_spec.rs`, `infra/auth.rs` and `app.rs` assume of a bare
  `String`/`&str`. A `BasePath` newtype constructible only through normalisation
- [ ] **Currency and month as strings.** `cost_base::Parcel::currency` is `&str` and the `infra::fx`
  lookups take `currency: &str` / `month: &str` (`'YYYY-MM'`) though CLAUDE.md types every currency
  as `CurrencyCode`. Take `&CurrencyCode` and a small `RateMonth` type
- [ ] **`FilterClauses::over` precondition.** "The statement must already be open for `AND …`" —
  have `over` push its own `WHERE 1=1` (or take the statement before `WHERE`) so the rule cannot be
  broken
- [ ] **`OpenParcel::remaining_as_of` "Always > 0".** Only `open_parcels::load` constructs it, so a
  positive-quantity type is low value; decide whether to type it or leave the comment
