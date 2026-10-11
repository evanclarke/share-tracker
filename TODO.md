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
- [x] **Unit basis of a quantity.** As-acquired and as-of/sale-date quantities are both `Decimal`;
  `open_parcels::db_units_sold`'s doc says the sale-date quantity "must" be re-based before it is
  subtracted, and `OpenParcel::remaining_as_of` vs `parcel.quantity` is distinguished only in prose
  (~180 comments state a basis). Introduce distinct types (e.g. `AcquiredUnits` / `UnitsAt`) with
  `adjustments::split_adjusted_quantity`/`as_acquired_quantity`/`sold_in_acquired_units` as the
  only conversions, carried by `UnitsConsumed::quantity` and `OpenParcel::remaining_as_of` — the
  quantity counterpart of `cost_base::Held`
  — Done 2026-10-11: `adjustments::AcquiredUnits` (a parcel's own basis — `ParcelRow`/`Parcel::
  quantity`, the pipeline's `units`, `AmitReductionEvent`'s four counts, `RolledParcel::remaining`)
  and `adjustments::UnitsAt`, which carries the date whose basis it is in (`UnitsConsumed::quantity`,
  `OpenParcel::remaining_as_of`, `RolledParcel::at_date_units`, the allocation reads), so a
  conversion is handed only its target date. The three functions plus `UnitsAt::rebased` are the
  only conversions, and now re-base in **either** direction — the one-directional window that
  `reduction_for` and the AMIT generator each special-cased by hand is gone (pinned by
  `a_quantity_re_bases_backward_to_an_earlier_date` and the split round-trip properties). Typing it
  exposed two sums over parcels of *different* as-acquired bases in `closing_price::held` (harmless
  non-zero tests, now `any`)
- [x] **Valuation-only FX fallback.** `fx::ValuationRate` exposes `pub rate`, so a caller can drop
  `provisional` ("the caller must surface this"), and valuation-only use is held by the
  `VALUATION_RATE_CALLERS_ALLOWED` file-name scan. Make the rate private and offer only an `apply`
  returning the value with its provisional flag attached; once tax paths accept only plain/`Aud`
  amounts, retire the allowlist scan
  — Done 2026-10-11: `ValuationRate`'s fields are private; `apply(amount)` and `quoted()` (the rate
  itself, which the period-performance report shows) return an `fx::Provisional<T>` whose only exit
  is `into_parts() -> (T, bool)`, so dropping the flag means naming it. `reports::valuation::
  ListingValuation` carries the `ValuationRate` in place of its separate `rate`/`provisional`
  fields (`provisional()` reads it), and period-performance's FX attribution is now two `apply`s.
  Pinned by `a_valuation_rate_hands_its_flag_to_everything_derived_from_it`. The allowlist scan
  **stays**: its retirement condition does not hold — the strict tax path (`FxRates::to_aud`)
  still returns a bare `Decimal`, so no type tells a tax figure from a valuation and only the scan
  keeps a tax report off the fallback
- [x] **`sale_trade_id: i64` overloaded.** `realised_gains::RealisedGainLoss::sale_trade_id` (and
  the tax report's `DisposalParcelRow` copy) is a trade id or a `rights_sales` id depending on the
  separate `source` field. Replace the pair with an enum (e.g. `DisposalId { Sell(TradeId),
  RightsSale(RightsSaleId) }`); the API has no external clients, so reshaping the JSON is allowed
  (update `docs/API.md`, and the CLAUDE.md note on the bare field)
  — Done 2026-10-11: `realised_gains::DisposalId { Sell(TradeId), RightsSale(RightsSaleId) }`
  (new `infra::id::RightsSaleId`, which `RightsSale::id` and its routes now carry too) replaces the
  pair on both rows, serialised adjacently tagged and flattened as `source` + `disposal_id`; the
  tax report's scrip-cash map is keyed by `TradeId` and its rights-sale map by `RightsSaleId`, so
  neither can be handed the other's id. The web UI had been linking every `sale_trade_id` cell to
  a *trade* — a rights sale's id opened whichever trade shared it; `disposal_id` now links per
  `source` (`disposalHref`, on Realised Gains and Net Capital Gain's disposals expand, which gained
  `links` support). Pinned by `db_rights_sale_flows_into_the_report` (rights sale 1 beside trade 1)
  and `a_disposal_id_links_to_the_record_its_source_names`
- [x] **Same overload in the health report's nil-proceeds alert** (found doing the item above):
  `reports::health`'s `nil_proceeds_disposals` rows carry `kind` (`Sell`/`RightsSale`) plus a bare
  `record_id: i64` whose table depends on it. Give it the same `DisposalId` (or its own enum), and
  update the `refreshHealthBanner` reader and `docs/API.md`
  — Done 2026-10-11: `NilProceedsDisposal` carries `realised_gains::DisposalId` (flattened as
  `source` + `disposal_id`, the realised-gains rows' own pair) in place of `kind` + `record_id`;
  `NilProceedsKind` is gone, the loader reads each half's id as its typed `TradeId`/`RightsSaleId`,
  and the same-date tie-break orders by the disposal instead of comparing ids across tables. The
  banner reads `d.source`/`d.disposal_id`. Pinned by the health tests (`DisposalId` assertions, and
  the served JSON carrying no `kind`/`record_id`) and `the_nil_proceeds_banner_reads_the_disposal_id`
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

## Re-basing a remainder through the as-acquired basis loses exactness (verification 2026-10-11)
Found while verifying the unit-basis types (`9fabf3d`), and present on `main` before them. After a
3-for-1 split, a 200-unit BHP parcel with one post-split sale of 200 units reports a remaining
quantity of `399.99999999999999999999999999`, not `400`. The pipeline converts each allocation back to
the as-acquired basis (200 ÷ 3 = 66.666…667, rounded to `Decimal`'s 28 digits), subtracts it, then
re-bases the 133.333…333 remainder forward (× 3), and the rounding survives the round trip. The
cost base pro-rated over that remainder (`5906.6333…`) carries the same last-digit error.
- [ ] Reproduce in a test: a split whose ratio does not divide the sold quantity (3-for-1, a sale of
  200 post-split units out of a 200-unit parcel), asserting `OpenParcel::remaining_as_of` is exactly
  `400` and the cost base exactly two-thirds of the parcel's
- [ ] Check whether the same rounding can refuse a legitimate sale: three post-split sales of 200 out
  of that parcel convert to 3 × 66.666…667, which sums past 200 as-acquired units, so the third
  could hit `PurchaseQuantityExceeded` (`sold_in_acquired_units` in the Sell/allocation capacity
  checks) or leave a residue that keeps a fully sold parcel open
- [ ] Fix it at the source rather than per caller: e.g. subtract in the later basis when every
  allocation is already in it, or carry the conversion as an exact fraction (multiply the
  numerators and denominators across sales, divide once) — so a remainder, a capacity check and a
  cost-base pro-rate all see the exact figure
