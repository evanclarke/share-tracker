# Done — Tax / CGT Domain Rules (ATO-cited)

## ATO worked-example acceptance tests
(API-level tests reproducing the worked examples from the ATO guidance mirrored in `docs/ato/` — each test cites its document + example, enters the facts purely via the HTTP API, and asserts the figures the ATO states. `src/ato_examples.rs`, a `#[cfg(test)]`-only module.)
- [x] `docs/ato/cgt-how-to-calculate.md` "Example: CGT with discount" (Justin: $10,000 gain held 18 months → declares $5,000) — `ato_examples::cgt_how_to_calculate_example_cgt_with_discount`
- [x] `docs/ato/cgt-how-to-calculate.md` "Example: working out CGT for a single asset" (Rhi's property: $530,000 all-in costs vs $600,000 → $70,000 gain, $35,000 net) — `ato_examples::cgt_how_to_calculate_example_single_asset`
- [x] `docs/ato/cgt-how-to-calculate.md` "Example: working out CGT for multiple assets" (adds the $4,500 share loss: losses before the discount → $65,500 → $32,750 net) — `ato_examples::cgt_how_to_calculate_example_multiple_assets`
- [x] `docs/ato/lic-capital-gain-deduction.md` "Example: Resident individual" (Ben: $70 franked, $30 credit, $25 LIC deduction in the FY2025 tax summary) — `ato_examples::lic_capital_gain_deduction_example_resident_individual`
- [x] `docs/ato/cgt-dividend-reinvestment-plans.md` "Example: dividend reinvestment plans" (Natalie: $360 dividend reinvested at $8 → 45 new shares acquired for $360 on 20 Dec 2024; the $360 stays assessable in FY2025) — `ato_examples::drp_example_natalie_reinvested_dividend`, driving DRP enrolment + `POST /income/:id/reinvest` + overview + tax summary
- [x] `docs/ato/cgt-keeping-records-shares.md` "Example: identifying when shares or units were acquired" (Boris nominates the 2024 $10 parcel for his 1,500-share sale at $8 → $3,000 capital loss in FY2025, keeping 1,000 @ $5 + 1,500 @ $10 = $20,000 cost base) — `ato_examples::keeping_records_example_boris_identifying_shares_sold`, driving specific parcel allocation via `PUT /sells`
- [x] `docs/ato/you-and-your-shares-dividends.md` Examples 1–2 (John: $700 franked + $200 unfranked + $300 credit → $1,200 total assessable dividend income in FY2025) — `ato_examples::you_and_your_shares_examples_1_2_john_assessable_dividend_income`
- [x] `docs/ato/you-and-your-shares-dividends.md` "Example 6" (Matthew: held < 45 days, credits > $5,000 → the $5,600 franking credits are denied) — un-ignored when the "Franking-credit entitlement rules" section landed; also asserts the denied amount is surfaced in `franking_credits_denied` (`ato_examples::you_and_your_shares_example_6_matthew_holding_period_rule`)
- [x] `docs/ato/cgt-non-assessable-payments.md` "Example 45" (Rob: 50c/share return of capital reduces the cost base to $4.50/share, no capital gain) — un-ignored when the ReturnOfCapital corporate action landed; the speculative `PUT /corporate_actions/1` entry API matched the implemented endpoint unchanged (`ato_examples::cgt_non_assessable_payments_example_45_rob_return_of_capital`)
- [ ] `docs/ato/cgt-cost-base.md` worked examples (capital works deduction on reduced cost base; recouped expenditure) — not reproducible by design: the "Reduced cost base and the five cost-base elements" clarification was RESOLVED 2026-06-07 as *document as a known limitation* (elements 3–5 and a distinct reduced cost base are not modelled), so these examples' facts cannot be entered
- [ ] `docs/ato/lic-capital-gain-deduction.md` "Example: Beneficiary of a trust or partner in partnership" — not reproducible by design: the "Taxpayer entity type" clarification was RESOLVED 2026-06-07 as *not modelled* (individual resident assumed; partnerships/trusts cannot be represented)
- [x] `docs/ato/share-splits-and-consolidations.md` (TD 2000/10) "Example 1" (John: 2-for-1 conversion → his 3,000 shares of 30 April 1988 at $1.00 become 6,000 at $0.50, acquisition dates preserved, no CGT event) — `ato_examples::td_2000_10_example_1_john_share_split`, driving the ShareSplit corporate action + open-parcels + net-capital-gain
- [x] `docs/ato/share-splits-and-consolidations.md` (TD 2000/10) "Example 2" (John, consolidation: 1-for-2 → 1,000 + 1,500 shares at $2.00, no CGT event) — `ato_examples::td_2000_10_example_2_john_share_consolidation`
- [x] `docs/ato/bonus-shares.md` "Example 35: Fully paid bonus shares" (Chris: 300 shares bought at $1 on 27 May 1986 + 300 bonus shares on 15 Nov 1986 → 600 shares at 50 cents, $300 cost base unchanged, acquisition date preserved, no CGT event; the 1 June 1985 parcel's pre-CGT exemption is out of scope and only its quantity/date asserted) — `ato_examples::bonus_shares_example_35_chris_fully_paid_bonus_shares`, driving the BonusIssue corporate action + open-parcels + net-capital-gain. Examples 36–37 (partly paid bonus shares, call payments, dividend-assessed issue on pre-CGT originals) not reproducible — noted in the `ato_examples` header
- [x] `docs/ato/rights-issues.md` "Example 40: Rights exercised" (Shanti: 500 rights at 1-for-4 over her ZAC shares, exercised 1 Aug 1998 at $1.80 — no CGT event; the new shares are acquired at exercise with cost base = the exercise payment. The post-CGT half is reproduced: her 1 Dec 1996 parcel's 250 rights → 250 shares at a $450 cost base acquired 1998-08-01; the pre-CGT half turns on pre-CGT originals, not modelled) — `ato_examples::rights_issues_example_40_shanti_rights_exercised`, driving the RightsIssue corporate action + `POST /corporate_actions/:id/exercise` + open-parcels + net-capital-gain. Example 39 (sale of the rights themselves, with the deemed acquisition date inherited from the original shares) is not reproducible — selling/lapsing rights is not modelled; noted in the `ato_examples` header
- [x] `docs/ato/demergers.md` (QC 64895) "Example 30: No pre-CGT interests" + "Example 32: Using the discount method after a demerger (1)" (Anita: 280 BHP Billiton shares, $2,500 cost base, 1-for-5 demerger of BHP Steel apportioning 94.937%/5.063% → BHP Billiton $2,373.425 / 56 BHP Steel shares at $126.575 (ATO shows the cent-rounded $2,373.43/$126.58), both keeping the 15 Aug 2001 acquisition date; a BHP Steel sale after 15 Aug 2002 — under 12 months after the demerger — is discount-eligible) — `ato_examples::demergers_examples_30_32_anita_bhp_billiton_demerger`, driving the Demerger corporate action + `POST /corporate_actions/:id/demerge` + open-parcels + net-capital-gain + realised-gains. Examples 31 and 33 (pre-CGT originals, the no-rollover arm) not reproducible — noted in the `ato_examples` header
- [x] `docs/ato/share-buy-backs.md` (QC 66049) "Example: off-market buy-back" (Ranjini: 1,000 of her 10,000 $6 shares sold into a $9.60 buy-back carrying a $1.40 franked dividend ($0.60 credit), market value $10.20 → capital proceeds $8,800, capital gain before discount $2,800, plus the $1,400 dividend + $600 credit in her return) — `ato_examples::share_buy_backs_example_ranjini_off_market_buy_back`, driving the BuyBack corporate action + `POST /corporate_actions/:id/participate` + realised-gains + tax-summary + overview
- [x] `docs/ato/you-and-your-shares-dividends.md` "Example 7" (Jessica: last-in-first-out identification for the 45-day rule — 4,000 of 14,000 entitled shares deemed sold under LIFO regardless of the CGT parcel allocation, so 4/14 of the credits are denied) — `ato_examples::you_and_your_shares_example_7_jessica_lifo_identification`
- [ ] `docs/ato/takeovers-and-scrip-for-scrip.md` Examples 26–28 (Desiree, Gunther, Stephanie) — not reproducible in this system: none matches the modelled full-rollover single-replacement-class exchange (26 is the no-rollover election — an ordinary market-value disposal entered manually; 27 is a partial rollover with cash; 28 exchanges into two replacement share classes with the cost base apportioned by market value). The modelled mechanics are covered by `scrip_exchange`/report unit tests; noted in the `ato_examples` header
- [ ] "Guide to foreign income tax offset rules 2025" Example 16 (Anna: $3,400 foreign tax limited to a $2,321 offset) — not reproducible in this system: the offset-limit calculation needs the taxpayer's full income-tax position (employment income, deductions, Medicare levy), which is outside the data model. The FITO section below covers only the $1,000 de-minimis cap computable from this system's data

## Capital-loss carry-forward across years
(REQUIREMENTS "Planned Enhancements — Capital-loss carry-forward across years". Net capital losses carry forward indefinitely and apply before the discount, per `docs/ato/cgt-using-capital-losses.md`. Today `net-capital-gain` computes the current year's `capital_loss_carried_forward` but never consumes a prior year's carried-forward loss in a later year, so post-loss years are overstated.)
- [x] Chain carried-forward losses across the year series in `/portfolio/net-capital-gain`: an unused net capital loss from one year is applied in the next year that has gains (non-discountable gains first, then discount-eligible, then halve the remainder) — `db_net_capital_gain` now walks the years ascending with a running brought-forward balance: each year nets gains against `capital_losses + capital_loss_brought_forward` (non-discountable first, losses always before the discount), and the unused excess (`capital_loss_carried_forward`) becomes the next year's brought-forward. New response field `capital_loss_brought_forward`; `capital_losses` remains only the losses arising that year
- [x] Add an enterable opening carried-forward capital loss (losses from before the first year in the system), stored as a recognised data-model value (not derived) and used as the starting balance — DB schema + migration (no data dropped) + write path — singleton `cgt_settings` table (migration `0006_cgt_settings.sql`, `CHECK (id = 1)` so at most one row), entity `src/entities/cgt_settings.rs` with GET/PUT/DELETE at `/cgt_settings(/:id)`; PUT rejects a negative amount or id ≠ 1 with 422; absent row reads as zero (`db_opening_capital_loss`), which seeds the report's loss chain. A CGT Settings CRUD view is added to the SPA `ENTITIES` config
- [x] Tests: an earlier-year loss reduces a later year's net capital gain; a loss fully absorbing later gains leaves zero assessable and carries the remainder forward; an entered opening loss balance is applied (`net_capital_gain::tests::db_earlier_year_loss_reduces_later_year_gain`, `db_loss_absorbing_later_gains_leaves_zero_and_carries_remainder`, `db_opening_capital_loss_is_applied_as_starting_balance`, `db_opening_loss_chains_through_a_loss_year_in_order`; plus `cgt_settings::tests` — CRUD round-trip with decimal precision, singleton CHECK, negative/non-singleton-id 422s, zero default — and `web::tests::cgt_settings_ui_present`)
- [x] README sync: net-capital-gain report description (cross-year carry + opening balance), schema/endpoint for the opening loss balance — Features bullet, web-frontend paragraph, `cgt_settings` in Database schema + the standalone-tables Relationships note, a CGT settings HTTP API section, the net-capital-gain computation/response-fields description, and the 422 response-code row

## Reduced cost base and the five cost-base elements
(REQUIREMENTS "Planned Enhancements — Reduced cost base and the five cost-base elements", `docs/ato/cgt-cost-base.md`.)
- [x] NEEDS CLARIFICATION: decide whether to model the ATO reduced cost base (for losses — excludes element 3, no indexation) as distinct from the cost base, or document the single-cost-base behaviour as a known limitation — RESOLVED 2026-06-07: **document as a known limitation**. Only elements 1–2 are captured, so cost base and reduced cost base are identical by construction; for listed shares elements 3–5 rarely apply (element-3 borrowing/holding costs are typically deductible instead and then excluded from the cost base anyway). Limitation stated in README (Known limitations section)
- [x] NEEDS CLARIFICATION: decide whether to capture cost-base elements beyond acquisition (1) and incidental/brokerage (2) — element 3 (ownership costs), 4 (capital improvements), 5 (title/defence costs) — RESOLVED 2026-06-07: **not captured** (same decision as above; out of scope for a listed-share tracker)
- [ ] If elements 3–5 in scope: model per-parcel additional cost-base costs (DB schema + migration) and include them in cost base (excluding element 3 from the reduced cost base) in the portfolio/unrealised/realised/net-capital-gain reports — N/A: decided out of scope (2026-06-07), see the resolution above
- [ ] Tests: additional cost-base costs flow into the cost base; element 3 is excluded from the reduced cost base used for losses — N/A: decided out of scope (2026-06-07)
- [x] README sync: cost-base composition in the report descriptions + any new schema — Known-limitations note in README: cost base = elements 1–2 only; cost base and reduced cost base treated as identical; no new schema (decision was to document, not model)

## Taxpayer entity type and CGT discount rate
(REQUIREMENTS "Planned Enhancements — Taxpayer entity type and CGT discount rate". Discount is currently hard-wired to the individual 50% rate.)
- [x] NEEDS CLARIFICATION: decide whether to introduce a taxpayer-entity concept (Individual, SMSF/complying super, Company, Trust/Partnership) driving the CGT discount rate (50% / 33⅓% / 0% / 50%) and the LIC deduction rate (`docs/ato/lic-capital-gain-deduction.md`) — RESOLVED 2026-06-07: **not modelled**; the user is an individual resident taxpayer, so the 50% rates stay hard-wired and the assumption is surfaced explicitly (the spec's fallback path)
- [ ] If entity type in scope: model it (DB schema + migration), drive the discount and LIC-deduction rates from it in `/portfolio/net-capital-gain` and the tax summary — N/A: decided out of scope (2026-06-07), see the resolution above
- [x] If not yet modelled: state the individual-resident 50% assumption explicitly in the report output and README — every `/portfolio/net-capital-gain` and `/portfolio/tax-summary` row carries an informational `taxpayer_basis` field stating the individual-resident 50% CGT discount / 50% LIC deduction assumption (flows into the CSV exports and the SPA tables automatically, as report columns derive from the response keys); README states the assumption in the report sections + a Known limitations section
- [x] Tests: discount/LIC rates vary correctly by entity type (or: the individual-resident assumption is surfaced) — the assumption is surfaced: `net_capital_gain::tests::db_rows_state_the_individual_resident_basis` (+ CSV export header/value assertions in `api_export_returns_csv_with_expected_columns`), `tax_summary::tests::db_rows_state_the_individual_resident_basis`

## Franking-credit entitlement rules
(REQUIREMENTS "Planned Enhancements — Franking-credit entitlement rules". `ex_date` is already captured and is the input the at-risk holding-period test needs. ATO worked examples mirrored in `docs/ato/you-and-your-shares-dividends.md`; Matthew's Example 6 test was un-ignored and Jessica's Example 7 added as part of this section — see the ATO worked-example section above.)
- [x] Apply the 45-day holding-period rule (90 days for preference shares) to decide whether a dividend's franking credits are claimable — `reports::franking::holding_period_test` walks the listing's trade history with the ATO-mandated **last-in first-out** share identification (independent of the CGT parcel allocation): units held when the shares go ex-dividend (`ex_date`, falling back to `date_paid` when not recorded) are entitled; an entitled unit sold with fewer than 45 at-risk days (acquisition and disposal days both excluded) within the qualification period (ex-date + 45 days) is disqualified and its proportional share of the credits denied (`credits × disqualified / entitled`, multiplied before dividing for precision). Preference shares need 90 days: new `listings.preference` boolean flag (migration `0007_listing_preference.sql`, additive — same flag pattern as `amit`), editable in the SPA listings view
- [x] Apply the $5,000 small-shareholder exemption (franking offsets up to $5,000/year claimable without the holding-period rule) — the tax summary totals each year's *attached* credits in AUD (income + AMMA; AMMA credits count toward the threshold but are never themselves denied, as an annual AMMA statement carries no per-distribution ex-date) and only runs the holding-period test in years at or above A$5,000 (`franking::small_shareholder_threshold_aud`; "below $5,000" is exempt, exactly $5,000 is not)
- [x] Tax summary reflects only claimable franking credits (or clearly flags credits at risk of disallowance), not all attached credits — `franking_credits` now excludes denied credits and the new `franking_credits_denied` response field surfaces the excluded amount (flows into the SPA tax-summary table automatically, as report columns derive from the response keys)
- [x] Tests: a dividend held under 45 days has its franking credits excluded; the small-shareholder exemption restores credits below the $5,000 threshold — `tax_summary::tests::db_franking_credits_denied_when_held_under_45_days`, `db_small_shareholder_exemption_keeps_credits_below_5000`, `db_exactly_5000_attached_credits_is_not_exempt`, `db_amma_credits_count_toward_small_shareholder_threshold`, `db_missing_ex_date_falls_back_to_date_paid`, `db_long_held_parcel_keeps_credits_in_non_exempt_year`; LIFO/at-risk-day/90-day mechanics in `reports::franking::tests` (parcel held through window qualifies, under-45-day sale disqualifies, end days excluded, LIFO picks the recent parcel, preference 90 days, pre-ex sales reduce entitlement, post-ex buys absorb sales first, DRP parcels count, zero-entitled denies nothing); `listing::tests::db_preference_flag_round_trips_and_defaults_false`; `web::tests::listing_management_ui_present` (preference field in the bundle); plus the Matthew + Jessica acceptance tests above
- [x] README sync: tax summary franking-credit treatment — Features bullet, `listings.preference` in the Database schema, and a franking-credit entitlement paragraph in the Tax summary report section

## Foreign income tax offset (FITO) cap
(REQUIREMENTS "Planned Enhancements — Foreign income tax offset (FITO) cap", `docs/ato/mytax-managed-funds.md`. Tax summary currently sums foreign tax with no cap.)
- [x] Apply the FITO limit: offsets above $1,000/year capped unless the full offset-limit calculation supports more — per `docs/ato/fito-limit.md` (Guide to FITO rules 2025, mirrored 2026-06-06): up to A$1,000/year of foreign tax is claimable with no offset-limit calculation; above that the limit calculation needs the taxpayer's full income-tax position (employment income, deductions, Medicare levy — outside this data model, per the Example 16 note in the ATO worked-example section). The tax summary therefore caps `foreign_tax_offsets` (income `foreign_tax_paid` + AMMA `foreign_tax_credits`, AUD, per year) at the A$1,000 de-minimis and surfaces the amount above it in the new `foreign_tax_offset_excess` response field — claimable only where the user's own offset-limit calculation supports more. The SPA tax-summary table picks up the new field automatically (report columns derive from response keys)
- [x] Tests: foreign tax under $1,000 passes through; above $1,000 is limited to the computed cap — `tax_summary::tests::db_foreign_tax_under_1000_passes_through`, `db_foreign_tax_exactly_1000_is_not_capped` ("up to $1,000" is uncapped), `db_foreign_tax_above_1000_is_capped_with_excess_surfaced` (Anna-shaped A$3,400 → 1,000 + 2,400 excess), `db_fito_cap_combines_income_and_amma_per_year` (income + AMMA combine before the test; each year capped independently)
- [x] README sync: tax summary FITO treatment — Features bullet + a FITO-cap paragraph in the Tax summary report section (`foreign_tax_offsets` semantics + `foreign_tax_offset_excess`); `docs/ato/fito-limit.md` indexed in `docs/ato/OVERVIEW.md`

## Corporate actions / additional CGT events
(REQUIREMENTS "Planned Enhancements — Corporate actions / additional CGT events". A1, E10, and G1 are modelled today.)
- [x] NEEDS CLARIFICATION: decide scope and data model for recording corporate actions per holding/parcel — RESOLVED (2026-06-06 data model, 2026-06-07 scope): data model is the `corporate_actions` table (`src/entities/corporate_action.rs`): one row per action against a *listing* (not per parcel — affected parcels are derived from holdings at the action date), with a CHECK-enforced `action_type` enum as the extension point for each further action type. Every action type enumerated below is now implemented (ReturnOfCapital, ShareSplit, BonusIssue, RightsIssue, BuyBack, ScripForScrip, Demerger); the enum remains the extension point for any future type
- [x] Share split / consolidation: adjust quantity and per-unit cost base, preserving total cost base and the original acquisition date for the discount — `ShareSplit` corporate action (TD 2000/10, mirrored in `docs/ato/share-splits-and-consolidations.md`; migration `0010_share_splits.sql` rebuilds `corporate_actions` via the rename pattern: nullable per-type payload columns + per-type CHECKs). On the conversion `date` every `split_old_units` units become `split_new_units` units (consolidation = new < old; one action type covers both). No CGT event: trade rows keep as-transacted quantities; reports and write-time checks re-base via `split_ratio`/`split_adjusted_quantity`/`as_acquired_quantity` (half-open interval — a trade dated on the conversion date is already post-split). Total cost base and acquisition date (the 12-month discount clock) are untouched; per-unit cost base scales inversely. Applied in: portfolio/unrealised/open-parcels (sold allocations re-based to as-acquired units; displayed quantities in current/as-of units), realised (allocation re-based for cost-base pro-rating), `PUT /sells` over-allocation check and `PUT /trades` shrink check (post-split sale units cover pre-split parcels), ReturnOfCapital interplay (`per_unit_reduction` + `g1_gains` scale per-unit payments across splits), and the franking 45-day LIFO walk (quantities normalised to one basis). Fractional consolidation remainders stay exact (no rounding/cash-in-lieu); AMIT adjustment quantities remain expressed in as-acquired units (documented on the field + README)
- [x] Bonus shares: new parcels with apportioned cost base — `BonusIssue` corporate action (ATO Guide to CGT "Bonus shares", mirrored in `docs/ato/bonus-shares.md`; migration `0011_bonus_issues.sql` rebuilds `corporate_actions` via the rename pattern, adding `bonus_units`/`bonus_held_units` + per-type CHECKs). On the issue `date` every `bonus_held_units` units held receive `bonus_units` additional units. The general (post-1 July 1998) non-assessable case: bonus shares take the original parcel's acquisition date and the parcel's cost base is apportioned over original + bonus shares — i.e. the no-CGT-event quantity re-base `(held + bonus)/held` with total cost base preserved, so BonusIssue rows fold into the split-event stream (`db_share_split_events`/`db_splits_for_listing` return the equivalent split new = held + bonus, old = held) and every report, write-time capacity check, ROC interplay, and the franking LIFO walk inherit the treatment with no further per-report code. A trade dated on the issue date is ex-bonus (same half-open interval as splits). Dividend-assessed bonus shares (chosen in lieu of a dividend) are out of scope here by design — they are a DRP trade (new parcel at issue date, cost base = dividend), already modelled; partly paid bonus shares / call payments not modelled (documented in `docs/ato/bonus-shares.md` + README)
- [x] Rights issues: new parcels with their cost-base treatment — `RightsIssue` corporate action (ATO Guide to CGT "Rights or options to acquire shares or units" + "Exercising rights or options", QC 64895, mirrored in `docs/ato/rights-issues.md`; migration `0012_rights_issues.sql` rebuilds `corporate_actions` via the rename pattern adding `rights_units`/`rights_held_units`/`exercise_price` + per-type CHECKs — `currency` is shared with ReturnOfCapital — and adds `trades.rights_action_id` FK). On the record `date` every `rights_held_units` units held earn the right to `rights_units` new units at `exercise_price` (a trade dated on the record date is ex-rights; recording the action changes nothing — free rights are NANE income on issue). The exercise operation (`POST /corporate_actions/:id/exercise`, `src/entities/rights_exercise.rs`, DRP-reinvestment pattern) atomically creates the new parcel as a Buy trade dated the exercise date — no CGT event; the 12-month discount clock runs from exercise — with cost base = exercise payment (qty × price) + `rights_cost` (the amount paid to acquire the rights, 0 if issued free; carried on the trade's brokerage column, numerically part of the single cost base everywhere). Write-time invariants: cumulative exercised units per action ≤ the entitlement from holdings at the record date (split-aware, fractional entitlements rounded up per registry practice; over-exercise → 422); exercise trades are immutable via `PUT /trades` (422 — delete frees the entitlement) and freeze their action against PUT/DELETE (422) while they exist. Out of scope, documented in `docs/ato/rights-issues.md`: selling/lapsing the rights themselves (Example 39's deemed-acquisition-date gain), pre-CGT originals, employee-scheme rights, retail premiums (entered as unfranked dividend income)
- [x] Return of capital (non-AMIT, CGT event G1): reduce cost base, distinct from the AMIT tax-deferred amount — `ReturnOfCapital` corporate action (entity CRUD at `/corporate_actions`, migration `0009_corporate_actions.sql`; PUT rejects a non-positive `amount_per_unit`, unknown listing/currency, or unrecognised action type with `422`). The per-unit payment reduces the cost base of parcels held on the payment date: open-holdings reports (portfolio/unrealised/open-parcels) net `amount_per_unit × remaining units` off each parcel acquired on/before the payment date; the realised report reduces an allocation only by payments dated within `[buy.date, sale.date]` (units sold before a payment were not held for it); all floor at nil. Excess over a parcel's per-unit cost base → CGT event G1 capital gain in the payment's income year (`net_capital_gain::g1_gains`, per-parcel date-ordered walk scaled to units still held; discount-eligible when held > 12 months at the payment date; AUD at the payment month's ATO rate, no manual fallback; G1 never produces a loss; new informational `cgt_event_g1_gain` response field + CSV column). New `return_of_capital_reduction` column in the open-parcels report. A payment's currency must match the trade's — reports fail loudly on a mismatch, never net across currencies. Web UI: Corporate Actions CRUD view (`corporate_actions` `ENTITIES` entry). The non-AMIT unit-trust E4 event is treated identically (the E4-at-sale timing nuance is not separately modelled; noted in `docs/ato/cgt-non-assessable-payments.md`)
- [x] Off-market share buy-back: split into capital and dividend components — `BuyBack` corporate action (ATO "Share buy-backs" QC 66049, mirrored in `docs/ato/share-buy-backs.md`; migration `0013_buy_backs.sql` rebuilds the whole trades-connected FK cluster via the rename pattern per 0012's note — corporate_actions, trades, income, parcel_allocations, amit_adjustments, attachments — widening the enum and adding `buyback_price`/`buyback_dividend`/`buyback_franking_credit`/`buyback_market_value` + per-type CHECKs and the provenance columns `trades.buyback_action_id` + `income.buyback_trade_id`). The action records the offer terms per unit: price, the dividend component of that price with its franking credit (both 0 for a listed-company buy-back announced after 7:30 pm AEDT 25 Oct 2022 — no dividend component), and optionally the market value had the buy-back not been proposed. Recording changes nothing; participating (`POST /corporate_actions/:id/participate`, `src/entities/buyback_participation.rs`) atomically creates the Sell — per-unit price = capital proceeds per unit = `max(price, market value) − dividend` (the ATO market-value rule), settlement = participation date, through the shared `sell::upsert_sell_in_tx` core so every /sells invariant holds — plus the dividend-component income row (`dividend × units` franked, `credit × units` credits; none when the dividend is 0), so realised/net-capital-gain pick up the CGT side and the tax summary + franking entitlement rules the dividend side with no special casing. Write-time integrity: the participation Sell is immutable via `PUT /sells` and `DELETE /sells` removes its income row with it; the income row is rejected by `PUT`/`DELETE /income`; `DELETE /trades` counts `income.buyback_trade_id` among blocking references; the action is frozen (PUT/DELETE → 422) while participations reference it. Out of scope, documented in `docs/ato/share-buy-backs.md`: corporate participating shareholders, revenue-account holdings
- [x] Merger / takeover / demerger incl. scrip-for-scrip rollover: parcel substitution carrying the original cost base and acquisition date — DONE for merger/takeover scrip-for-scrip rollover (2026-06-07): `ScripForScrip` corporate action (Subdiv 124-M, mirrored in `docs/ato/takeovers-and-scrip-for-scrip.md`; migration `0014_scrip_for_scrip.sql` rebuilds the trades-connected FK cluster via the rename pattern, widening the enum and adding `scrip_listing_id`/`scrip_new_units`/`scrip_old_units` + per-type CHECKs incl. `scrip_listing_id <> listing_id`, plus `trades.scrip_action_id` and `trades.deemed_acquisition_date`). The action records the exchange terms: on the exchange date every `scrip_old_units` units of the original listing become `scrip_new_units` units of the replacement listing. Recording changes nothing; exchanging (`POST /corporate_actions/:id/exchange`, `src/entities/scrip_exchange.rs`) atomically creates a closing Sell on the original listing (price 0, allocations consuming every open parcel, via the shared sell core; excluded from realised-gains/net-capital-gain — the rollover disregards the gain and the zero proceeds never surface as a loss) plus one replacement Buy per consumed parcel (dated the exchange date, so the replacement listing's later splits/ROC apply only from then; remaining reduced cost base — AMIT/ROC-adjusted — carried exactly on the brokerage column with price 0; the parcel's acquisition date — chained through earlier exchanges — carried as `deemed_acquisition_date`, which drives the 12-month discount clock and the AUD translation month in every report, preserving the original AUD cost base). Write-time integrity: the group is immutable trade-by-trade (PUT /sells, PUT/DELETE /trades → 422), `DELETE /sells` on the closing Sell removes the whole group (422 while a replacement Buy is consumed), the action is frozen while referenced, and the exchange rejects (422) a non-scrip action / already-exchanged / nothing-held / original-listing trades dated on or after the exchange date. Out of scope, documented in `docs/ato/takeovers-and-scrip-for-scrip.md`: no-rollover takeovers (manual Sell + Buy at market value), partial rollover with cash, multiple replacement classes, pre-CGT originals, loss rollovers (not permitted by law). DONE for demerger rollover (Div 125, 2026-06-07): `Demerger` corporate action (mirrored in `docs/ato/demergers.md`, QC 64895; migration `0015_demergers.sql` rebuilds the trades-connected FK cluster via the rename pattern, widening the enum and adding `demerger_listing_id`/`demerger_new_units`/`demerger_held_units`/`demerger_cost_base_pct` + per-type CHECKs incl. `demerger_listing_id <> listing_id`, plus `trades.demerger_action_id`). The action records the terms against the head entity's listing: on the demerger date every `demerger_held_units` units held receive `demerger_new_units` units of the demerged listing, with `demerger_cost_base_pct` percent of each parcel's cost base apportioned to the new interests (the head-entity-advised step 2 percentage; 0 < pct < 100). Recording changes nothing; demerging (`POST /corporate_actions/:id/demerge`, `src/entities/demerger.rs`) atomically creates a closing Sell on the head listing (price 0, consuming every open parcel via the shared sell core; excluded from realised-gains/net-capital-gain — the rollover disregards any gain) plus, per consumed parcel, a head replacement Buy carrying `(100 − pct)%` of its remaining reduced cost base and a demerged-entity Buy carrying `pct%` (the two legs sum exactly; quantities = remaining units and units × ratio, exact fractional entitlements kept), both dated the demerger date with the parcel's acquisition date carried as `deemed_acquisition_date` (the head dates are unchanged by law; the new interests' discount clock runs from the original acquisition — ATO Example 32 — and the AUD translation month is preserved). The head shares are never actually disposed of, so the closing Sell + head replacement Buys are excluded from the franking 45-day LIFO walk (original parcels keep their at-risk days; the demerged-entity Buys are included). Write-time integrity mirrors ScripForScrip: the group is immutable trade-by-trade (PUT /sells, PUT/DELETE /trades → 422), `DELETE /sells` on the closing Sell removes the whole group (422 while a replacement Buy is consumed), the action is frozen while referenced, and the demerge rejects (422) a non-demerger action / already-demerged / nothing-held / head-listing trades dated on or after the demerger date. Out of scope, documented in `docs/ato/demergers.md`: no-rollover demergers, pre-CGT originals, assessable demerger dividends / separate capital returns, registry cash-in-lieu of fractional entitlements
- [x] Security identity continuity across a ticker/name change, so a renamed listing's parcels are not orphaned — already structurally guaranteed and now locked in by tests + documented: listings are keyed by the surrogate `id` and **nothing is keyed by ticker** (every trade/income/AMMA/enrolment/corporate-action row references `listing_id`; reports resolve the ticker by join at read time), so a rename is an in-place edit (`PUT /listings/:id`, same id, new `ticker`/`name`) and the full history — parcels, cost bases, acquisition dates (the 12-month discount clock) — stays attached. README Listings section documents the in-place-edit path (and that a new listing must not be created for a renamed security; a merger/takeover relisting is a parcel substitution, not a rename — see the item above). Tests: `open_parcels::tests::db_ticker_rename_keeps_parcels_attached_to_the_listing` (parcel survives with the new ticker, unchanged acquisition date + cost base), `realised_gains::tests::db_sale_after_ticker_rename_keeps_cost_base_and_discount_clock` (post-rename sale allocates against the pre-rename parcel; discount clock runs from the original acquisition date)
- [x] Tests: each modelled action produces the correct adjusted parcels, cost base, and preserved acquisition date — DONE for ReturnOfCapital (`corporate_action::tests` CRUD/validation/per-unit-reduction helpers; `portfolio::tests::db_return_of_capital_reduces_cost_base`, `db_return_of_capital_before_acquisition_does_not_apply`, `db_return_of_capital_floors_cost_base_at_nil`; `unrealised_gains::tests::db_return_of_capital_reduces_cost_base`; `open_parcels::tests::db_return_of_capital_reduction_reported_and_netted_off`, `db_return_of_capital_floors_remaining_cost_base_at_nil`; `realised_gains::tests::db_return_of_capital_during_holding_reduces_cost_base`, `db_return_of_capital_after_sale_does_not_affect_cost_base`; `net_capital_gain::tests::db_g1_*` (excess gain, no-gain-within-cost-base, discount-eligible, accumulates-across-payments, scales-to-units-held); `web::tests::corporate_actions_ui_present`; plus Rob's acceptance test above) and DONE for ShareSplit (`corporate_action::tests`: split CRUD round-trip, mixed-payload CHECK/422s, `split_ratio_covers_half_open_interval`, `split_adjusted_and_as_acquired_quantities_are_inverse`, `per_unit_reduction_scales_payments_across_a_split`; `portfolio::tests::db_share_split_adjusts_quantity_and_preserves_cost_base`, `db_consolidation_shrinks_quantity_and_preserves_cost_base`, `db_post_split_sell_nets_off_pre_split_parcel`, `db_split_before_acquisition_does_not_apply`, `db_return_of_capital_after_split_scales_to_post_split_units`; `unrealised_gains::tests::db_share_split_adjusts_quantity_and_keeps_acquisition_date`; `open_parcels::tests::db_share_split_rebases_remaining_quantity_and_preserves_cost_base`, `db_consolidation_rebases_remaining_quantity`; `realised_gains::tests::db_post_split_sale_uses_unchanged_total_cost_base`, `db_partial_post_split_sale_pro_rates_cost_base`, `db_split_preserves_acquisition_date_for_discount`, `db_return_of_capital_after_split_reduces_sold_cost_base`; `net_capital_gain::tests::db_g1_payment_after_split_scales_to_post_split_units`; `sell::tests::db_post_split_sell_allocates_against_pre_split_parcel`; `trade::tests::db_shrink_check_rebases_post_split_allocations`; `franking::tests::db_split_between_buy_and_ex_date_compares_in_one_basis`; `web::tests::corporate_actions_ui_present`; plus John's TD 2000/10 acceptance tests above) and DONE for BonusIssue (`corporate_action::tests`: `db_insert_and_retrieve_bonus_issue_preserves_ratio`, `db_check_rejects_mixed_payloads` extended with bonus/split cross-payloads, `db_split_events_include_bonus_issues_as_equivalent_splits` (1-for-10 bonus → 11-for-10 re-base, interleaved with a real split in date order, ROC excluded), `api_bonus_issue_round_trip`, `api_invalid_bonus_issue_payloads_return_422`; `portfolio::tests::db_bonus_issue_adds_units_and_apportions_cost_base`; `realised_gains::tests::db_post_bonus_issue_sale_apportions_cost_base_and_keeps_discount` (partial post-bonus sale pro-rates the unchanged cost base; discount clock runs from the original buy); plus Chris's Example 35 acceptance test — `ato_examples::bonus_shares_example_35_chris_fully_paid_bonus_shares`) and DONE for RightsIssue (`corporate_action::tests`: `db_insert_and_retrieve_rights_issue_preserves_terms`, `db_rights_issue_is_not_a_split_or_payment_event` (recording it adjusts no parcel), `db_check_rejects_mixed_payloads` extended with rights cross-payloads, `api_rights_issue_round_trip`, `api_invalid_rights_issue_payloads_return_422`; `rights_exercise::tests`: `exercise_creates_a_buy_parcel_at_the_exercise_date` (acquired/settled at exercise, cost base = exercise payment), `rights_cost_is_part_of_the_new_parcels_cost_base`, `cumulative_exercises_cannot_exceed_the_entitlement`, `entitlement_reflects_holdings_at_the_record_date` (pre-record sells reduce it; a record-date buy is ex-rights), `split_before_the_record_date_rebases_the_entitlement`, `fractional_entitlements_round_up`, `invalid_exercises_are_rejected_and_nothing_persisted`, `exercise_trade_is_immutable_via_put_trades_but_deletable`, `referenced_action_cannot_be_edited_or_deleted`, `discount_clock_runs_from_the_exercise_date` (sale >12mo after the original buy but <12mo after exercise is non-discountable), plus API tests; `web::tests::corporate_actions_ui_present` extended; plus Shanti's Example 40 acceptance test — `ato_examples::rights_issues_example_40_shanti_rights_exercised`) and DONE for BuyBack (`corporate_action::tests`: `db_insert_and_retrieve_buy_back_preserves_terms` (incl. optional market value round-trip), `db_buy_back_is_not_a_split_or_payment_event` (recording it adjusts no parcel), `db_check_rejects_mixed_payloads` extended with buy-back cross-payloads, `api_buy_back_round_trip` (incl. the zero-dividend listed shape with defaults), `api_invalid_buy_back_payloads_return_422`; `buyback_participation::tests`: `participation_splits_capital_proceeds_from_the_dividend` (Ranjini-shaped: $8.80/unit proceeds via the market-value rule + $1,400/$600 income linked to the Sell), `market_value_only_ever_lifts_capital_proceeds` (MV below the price ignored; omitted MV uses the price), `no_dividend_component_creates_no_income_row`, `realised_gain_uses_the_capital_proceeds` ($2,800 discount-eligible gain), `invalid_participations_are_rejected_and_nothing_persisted` (404/not-a-buy-back/before-date/non-positive units/allocation mismatch/over-allocation, all rolled back), `participation_sell_is_immutable_and_deletes_with_its_income` (PUT /sells 422; PUT/DELETE /income 422; DELETE /trades Referenced; DELETE /sells removes Sell + allocations + income), `referenced_action_cannot_be_edited_or_deleted`, plus API tests; `web::tests::corporate_actions_ui_present` extended; plus Ranjini's acceptance test — `ato_examples::share_buy_backs_example_ranjini_off_market_buy_back`) and DONE for ScripForScrip (`corporate_action::tests`: `db_insert_and_retrieve_scrip_for_scrip_preserves_terms`, `db_scrip_for_scrip_is_not_a_split_or_payment_event` (recording it adjusts no parcel), `db_check_rejects_self_exchange`, `db_check_rejects_mixed_payloads` extended with scrip cross-payloads, `api_scrip_for_scrip_round_trip`, `api_invalid_scrip_for_scrip_payloads_return_422` (incl. same-listing and unknown replacement listing); `scrip_exchange::tests`: `exchange_substitutes_parcels_carrying_cost_base_and_acquisition_date`, `partly_sold_parcel_carries_only_the_remaining_cost_base`, `amit_and_roc_reductions_carry_into_the_replacement_cost_base`, `split_before_the_exchange_rebases_the_exchanged_units`, `chained_exchange_carries_the_original_acquisition_date`, `invalid_exchanges_are_rejected_and_nothing_persisted`, `a_second_exchange_of_the_same_action_is_rejected`, `exchange_trades_are_immutable_individually`, `deleting_the_closing_sell_removes_the_whole_group` (incl. blocked-while-consumed and the restored holding), `referenced_action_cannot_be_edited_or_deleted`, plus API tests; reports: `realised_gains::tests::db_scrip_exchange_closing_sell_is_excluded`, `db_sale_of_replacement_parcel_uses_carried_cost_base_and_combined_period`, `db_replacement_sale_within_combined_12_months_is_not_discounted`, `db_usd_replacement_cost_base_converts_at_the_original_buy_month`; `open_parcels::tests::db_scrip_replacement_parcel_reports_carried_date_and_cost_base`; `portfolio::tests::db_scrip_exchange_moves_holding_to_replacement_listing`; `unrealised_gains::tests::db_scrip_replacement_discount_counts_the_combined_period`; `net_capital_gain::tests::db_scrip_rollover_disregards_the_exchange_and_taxes_the_later_sale`; `web::tests::corporate_actions_ui_present` extended; no ATO acceptance test — Examples 26–28 don't match the modelled full-rollover single-class case, noted in the `ato_examples` header) and DONE for Demerger (`corporate_action::tests`: `db_insert_and_retrieve_demerger_preserves_terms` (sub-unit 5.063% pct round-trips exactly), `db_demerger_is_not_a_split_or_payment_event` (recording it adjusts no parcel), `db_check_rejects_self_demerger`, `db_check_rejects_mixed_payloads` extended with demerger cross-payloads, `api_demerger_round_trip`, `api_invalid_demerger_payloads_return_422` (incl. pct at/outside (0,100), same-listing, unknown demerged listing, stray currency); `demerger::tests`: `demerge_apportions_cost_base_and_carries_acquisition_dates`, `apportionment_keeps_the_total_cost_base_exact` (Anita-shaped 5.063% — the two legs sum exactly to the original), `partly_sold_parcel_carries_only_the_remaining_cost_base`, `amit_and_roc_reductions_reduce_the_apportioned_cost_base`, `split_before_the_demerger_rebases_the_units`, `invalid_demerges_are_rejected_and_nothing_persisted`, `a_second_demerge_of_the_same_action_is_rejected`, `demerge_trades_are_immutable_individually`, `deleting_the_closing_sell_removes_the_whole_group` (incl. blocked-while-consumed and the restored holding), `referenced_action_cannot_be_edited_or_deleted`, plus API tests; reports: `realised_gains::tests::db_demerger_closing_sell_is_excluded`, `db_post_demerger_sales_use_apportioned_cost_bases_and_combined_period`; `portfolio::tests::db_demerger_splits_holding_across_listings`; `open_parcels::tests::db_demerger_parcels_report_carried_date_and_apportioned_cost_base`; `net_capital_gain::tests::db_demerger_rollover_disregards_the_demerge_and_taxes_the_later_sales`; `franking::tests::db_demerger_artifact_trades_keep_at_risk_days_running`; `web::tests::corporate_actions_ui_present` extended; plus Anita's Examples 30+32 acceptance test — `ato_examples::demergers_examples_30_32_anita_bhp_billiton_demerger`). Every modelled action type now has its tests
- [x] README sync: new entities/endpoints and their schema + relationships — DONE for ReturnOfCapital (Features bullet, `corporate_actions` schema + Relationships, Corporate actions HTTP API section, cost-base notes on the overview/open-parcels/realised reports, CGT event G1 paragraph + `cgt_event_g1_gain` field in the net-capital-gain section, 422 row, web-frontend view list) and DONE for ShareSplit (Features bullet, per-type schema columns + CHECK notes, ShareSplit paragraph + example body + per-type 422 cases in the Corporate actions API section, quantity-basis notes on the overview/open-parcels/unrealised/realised reports and the Sells/trade-edit sections, AMIT-adjustment quantity-basis note, 422 row; `docs/ato/share-splits-and-consolidations.md` indexed in `docs/ato/OVERVIEW.md`) and DONE for BonusIssue (Features bullet, `bonus_units`/`bonus_held_units` schema columns + widened enum/CHECK notes, BonusIssue paragraph + example body + per-type 422 cases in the Corporate actions API section, the re-basing notes on the trade-edit/Sells/overview/open-parcels/unrealised/realised sections extended to cover bonus issues, the 422 row; `docs/ato/bonus-shares.md` indexed in `docs/ato/OVERVIEW.md`) and DONE for RightsIssue (Features bullet, `rights_units`/`rights_held_units`/`exercise_price` schema columns + widened enum/CHECK/currency notes, `trades.rights_action_id` schema row + Relationships line, RightsIssue paragraph + example body + an "Exercising a rights issue" subsection (endpoint, cost-base/discount-clock semantics, entitlement cap, immutability/freeze rules, 201/404/422) in the Corporate actions API section, the rights-exercise immutability bullet in the Trades section, the Exercise action in the web-frontend view list, the 201 and 422 Response-code rows; `docs/ato/rights-issues.md` indexed in `docs/ato/OVERVIEW.md`) and DONE for BuyBack (Features bullet, `buyback_*` schema columns + widened enum/CHECK/currency notes, `trades.buyback_action_id` + `income.buyback_trade_id` schema rows + Relationships lines, BuyBack paragraph + example body + per-type 422 cases in the Corporate actions API section, a "Participating in a buy-back" subsection (endpoint, capital-proceeds/market-value semantics, dividend income row, provenance/immutability rules, 201/404/422), the buy-back notes on the Trades/Income/Sells sections, the Participate action in the web-frontend view list, the 201 and 422 Response-code rows; `docs/ato/share-buy-backs.md` indexed in `docs/ato/OVERVIEW.md`) and DONE for ScripForScrip (Features bullet, `scrip_*` schema columns + widened enum/CHECK notes, `trades.scrip_action_id` + `trades.deemed_acquisition_date` schema rows + Relationships lines, ScripForScrip paragraph + example body + per-type 422 cases in the Corporate actions API section, an "Exchanging a scrip-for-scrip takeover" subsection (endpoint, closing-Sell/replacement semantics, deemed acquisition date, group immutability/freeze rules, 201/404/422), the exchange-group notes on the Trades/Sells sections, the rollover notes on the open-parcels/unrealised/realised/net-capital-gain report sections, the ScripForScrip pointer in the Listings ticker-rename note, the Exchange action in the web-frontend view list, the 201 and 422 Response-code rows; `docs/ato/takeovers-and-scrip-for-scrip.md` indexed in `docs/ato/OVERVIEW.md`) and DONE for Demerger (Features bullet, `demerger_*` schema columns + widened enum/CHECK notes, `trades.demerger_action_id` schema row + Relationships lines, Demerger paragraph + example body + per-type 422 cases in the Corporate actions API section, a "Demerging" subsection (endpoint, closing-Sell/head-and-demerged-replacement semantics, exact percentage apportionment, deemed acquisition date, the franking-walk exclusion, group immutability/freeze rules, 201/404/422), the group notes on the Trades/Sells sections, the rollover notes on the open-parcels/unrealised/realised/net-capital-gain report sections and the franking paragraph in the tax summary, the Demerge action in the web-frontend view list, the 201 and 422 Response-code rows; `docs/ato/demergers.md` indexed in `docs/ato/OVERVIEW.md`). Every modelled action type's README sync is done

## Worthless / delisted shares — capital loss on a company in liquidation (CGT events G3 and C2)
(REQUIREMENTS "Worthless / delisted shares" 2026-06-08. A failed/liquidated/deregistered holding's capital loss must be recognisable without an ordinary sale; today the dead parcel stays open forever and the loss never reaches the gains reports. A capital loss — never income, never discounted — flowing through the existing loss-netting + carry-forward. ATO: CGT event G3 s104-145 / TD 2000/52 — liquidator's written declaration, opt-in, loss = reduced cost base, cost base then reset to nil; CGT event C2 s104-25 / TD 2000/7 — actual cancellation/deregistration disposal at (usually nil) proceeds. New corporate-action type, reusing the closing-Sell/group machinery built for scrip-for-scrip and demergers but *recognising* the loss instead of disregarding it. Cross-ref DONE.md "Corporate actions / additional CGT events" — G3/C2 are not among the modelled types.)
- [x] Mirror the ATO worthless-shares guidance into `docs/ato/` (the "Investments in a company in liquidation or administration" page + TD 2000/52 (G3) and TD 2000/7 (C2 on deregistration); source URL + retrieval date header), and index it in `docs/ato/OVERVIEW.md` — `docs/ato/worthless-shares.md` (QC 52234 + TD 2000/52 + TD 2000/7, retrieved 2026-06-08 via `scripts/ato-fetch.py`); OVERVIEW.md indexes it in the CGT-mechanics table. The Dave worked example is mirrored and cited
- [x] New corporate-action type via a migration (rename pattern, per the existing corporate-action migrations): widen the `corporate_actions` `action_type` enum and add the per-type payload (the event date is the existing `date`; an event-kind discriminator `G3Declaration` vs `C2Cancellation`, or two action types) + per-type CHECKs + the provenance column on `trades` for the closing Sells — `migrations/0023_worthless_shares.sql` rebuilds the FK cluster (the first corporate-action migration after the 0019 snapshot triggers, so it recreates them); added `action_type='WorthlessShares'`, a CHECK-enforced `worthless_event` enum column (`G3Declaration`/`C2Cancellation`) with the per-type CHECK, and `trades.worthless_action_id` FK. `ActionKind::WorthlessShares { worthless_event: WorthlessEvent }` in `corporate_action.rs`
- [x] The recognise operation (`POST /corporate_actions/:id/...`, DRP-reinvestment/scrip pattern): atomically close every open parcel of the listing held at the event date through a provenance-marked Sell at **nil proceeds** via the shared sell core, each parcel producing a capital loss equal to its remaining reduced cost base (cost base after AMIT / return-of-capital reductions; = cost base under the elements 1–2 limitation) — `POST /corporate_actions/:id/recognise` (`src/entities/worthless.rs`) builds the closing Sell via `sell::upsert_sell_in_tx` (new `worthless_action_id` param) consuming every open parcel across every account; the loss itself is computed by the realised-gains report from the nil proceeds
- [x] The recognised losses reach the realised-gains report (as `capital_loss`) and the net-capital-gain report's loss pool — confirm they net ATO-optimally and carry forward like any realised loss; a capital loss is never discounted, so no 12-month/discount-eligibility handling — the closing Sell carries `worthless_action_id` but, unlike `scrip_action_id`/`demerger_action_id`, is **not** excluded by `db_realised_gains`, so its nil proceeds against the cost base surface as a `capital_loss` that `net_capital_gain` consumes unchanged (tested: `recognise_closes_parcels_and_records_the_capital_loss`, `recognised_loss_feeds_the_net_capital_gain_loss_pool`)
- [x] Write-time integrity (mirror scrip/demerger groups): group trades immutable individually (PUT/DELETE → 422); deleting the operation restores the pre-event holding (blocked while a closing Sell is drawn on); the action frozen (PUT/DELETE → 422) while referenced; the operation rejects (422) a wrong action type, an already-recognised action, or nothing held at the event date — `sell::SellError::WorthlessSell` (PUT /sells), `trade::db_delete` refuses (DELETE /trades), `DELETE /sells` restores the holding and thaws the action, `corporate_action` freeze includes `worthless_action_id`; the operation rejects NotWorthlessShares/AlreadyRecognised/NothingHeld/TradedOnOrAfterEventDate and rolls back (tested in `entities::worthless::tests`)
- [x] Web UI: the action + operation render through the Corporate Actions `ENTITIES` view and a new `ACTIONS` descriptor (no bespoke screen); asserted present in the served bundle — `WorthlessShares` action_type + `worthless_event` field group + typeDesc + 'Event date' label + Recognise rowAction; the `recognise` confirm-only ACTIONS entry; `web::tests::corporate_actions_ui_present` / `post_actions_are_config_driven` / `corporate_action_form_is_split_by_type`
- [x] Tests: G3 declaration and C2 cancellation each close the held parcels and produce the correct capital loss in realised-gains + net-capital-gain; the loss nets/carries forward; over/again/nothing-held/wrong-type rejected and rolled back; group immutability + delete-restores-holding; action freeze while referenced; web bundle assertion; an `ato_examples.rs` acceptance test for any representable worked example — 11 inline tests in `entities::worthless`, 4 in `entities::corporate_action`, and `ato_examples::worthless_shares_example_dave_capital_loss_on_dissolution` (Dave, QC 52234 — 1,000 × $1.70 = $1,700 capital loss)
- [x] Docs: `docs/SCHEMA.md` (new columns + CHECKs + Relationships), `docs/API.md` (the action, the operation endpoint, 201/404/422 causes, Response-codes rows), README Features list (capital-loss recognition on a liquidated/delisted holding) — `corporate_actions.worthless_event` + `trades.worthless_action_id` + the Relationships line in SCHEMA.md; the `WorthlessShares` bullet, the recognise operation table row + section, the PUT/DELETE-trades/sells 422 notes, the 201/422 Response-codes rows in API.md; the Worthless/delisted-shares feature bullet in the README

## Trust distribution income year — present entitlement (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/trust-income-timing.md`, QC 23087. The one correctness gap from the BA review: a July-paid June trust distribution is currently attributed to the wrong FY.)

- [x] Migration: nullable `entitlement_date` (TEXT date) on `income`; supplying it on a non-trust row (`trust_income` false) is rejected 422 at write time — migration `0003_income_entitlement_date.sql` (ADD COLUMN with a cross-column CHECK: NULL unless `trust_income = 1`); `entities::income` gains the field on model + body, `UpsertError::EntitlementDateOnNonTrust` rejected before the write with an actionable 422 body ("entitlement_date only applies to trust distributions…"). Tests: `income::tests::db_entitlement_date_on_non_trust_rejected`, `api_entitlement_date_on_dividend_returns_422_with_detail`, round-trips `db_entitlement_date_round_trips_on_trust_row` / `api_trust_entitlement_date_round_trips`
- [x] Tax summary + CSV export: a trust row with `entitlement_date` has **every** component attributed to the FY of that date (fall back to `date_paid` when absent; non-trust rows unchanged) — `db_tax_summary` computes an assessment date per income row (`entitlement_date` when `trust_income` and present, else `date_paid`) driving both the FY bucket and the AUD-conversion month (`aud_field`); the CSV export shares the same records, no column change. Tests: `tax_summary::tests::db_trust_distribution_assessed_by_entitlement_date_not_payment` (trust → FY2026, same-day dividend → FY2027, TFN withholding follows too), `db_trust_without_entitlement_date_assessed_by_date_paid`, `db_trust_entitlement_date_drives_fx_month` (June-only USD rate converts; a July-keyed lookup would fail loudly)
- [x] Franking 45-day at-risk test keeps anchoring on `ex_date`/`date_paid`; the A$5,000 small-shareholder threshold year follows the row's assessment year — the `FrankedDividend` walk still anchors `ex_date` (fallback `date_paid`) but its `tax_year` is the assessment year, so July-paid June trust credits count toward the entitlement year's threshold. Test: `tax_summary::tests::db_franking_threshold_year_follows_entitlement_date` (4,990 + 20 credits cross A$5,000 only because the trust row lands in FY2026; long-held parcel passes the walk, nothing denied)
- [x] Web UI: the income form's Trust distribution selection reveals the entitlement-date field (defaulting to the pay date); included in the advanced field set — `entitlement_date` `dt` field on the income `ENTITIES` entry (hint explains present entitlement), added to `INCOME_ADVANCED_FIELDS`; `applyEntitlement` in `wireIncomeEntry` reveals the field in simple mode when Trust is selected, prefilling from the pay date, and `transformBody` clears it when the mode is switched away so the server's trust-only 422 can't be tripped by a leftover value. Test: `web::tests::income_entitlement_date_ui_present`
- [x] Tests: July-paid June trust distribution reaches the earlier FY; a dividend paid the same day is unchanged; 422 on a non-trust row; threshold-year test — all listed above; full suite 749 passing, build warning-free
- [x] Docs: `docs/SCHEMA.md` (column), `docs/API.md` (attribution rule + 422), README tax-summary feature text — SCHEMA.md income table row + currency-conversion note; API.md Income "Entitlement date" paragraph, tax-summary attribution sentence, Response-codes 422 cause; README Income-recording + Tax-summary feature bullets

## Non-AMIT trust tax-deferred amounts — CGT event E4 cross-check (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/cgt-non-assessable-payments.md` (E4), `docs/ato/amit-cost-base-adjustments.md` (AMIT treatment unchanged).)

- [x] Migration: optional informational `tax_deferred_amount` (TEXT decimal, ≥ 0) on `income`; trust rows only (422 otherwise); no calculation uses it — the E4 reduction itself stays the `ReturnOfCapital` corporate action
  - Done: `migrations/0004_income_tax_deferred_amount.sql` (column CHECK mirrors 0003's entitlement-date pattern: NULL unless `trust_income = 1` and the cast value ≥ 0); `entities::income` carries the field through model/body/FromRow/upsert and rejects non-trust (`TaxDeferredOnNonTrust`) and negative (`TaxDeferredNegative`) values at write time with actionable 422 messages
- [x] Non-blocking report (pattern: settlement-holiday coverage): trust income rows with a non-zero `tax_deferred_amount` whose listing has no `ReturnOfCapital` action dated in the row's FY; entering the action clears the flag
  - Done: `reports::e4_cross_check` (`GET /reports/e4_cross_check`), both inputs on one read transaction; the row's FY is its assessment year (`entitlement_date` when set, else `date_paid` — the tax summary's attribution rule). En route, the four inline July-cutoff FY computations (tax summary ×3, net capital gain) were consolidated into the new shared `domain::tax_year::tax_year_for`, which the report also uses — one definition of the FY bucketing rule
- [x] Web UI: advanced income field + standard `REPORTS` entry
  - Done: `tax_deferred_amount` joins `INCOME_ADVANCED_FIELDS` (a stored value forces the form open in advanced mode; switching simple mode away from Trust clears it alongside `entitlement_date`); REPORTS gains the `e4-cross-check` entry; the column was already classified money in `COLUMN_KINDS` (shared with the AMMA field of the same name)
- [x] Tests: flagged / cleared-by-action / omitted cases; the 422; report API test
  - Done: entity round-trip / non-trust 422 (DB + API with detail) / negative 422 / omitted-stays-NULL in `entities::income`; flagged / same-FY-action-clears / other-FY and other-listing don't clear / NULL-or-zero omitted / entitlement-date-governs-FY / API in `reports::e4_cross_check`; `web::tests::income_tax_deferred_e4_ui_present`; `domain::tax_year` boundary test
- [x] Docs: `docs/SCHEMA.md`, `docs/API.md` (report + 422 + Response codes), README
  - Done: SCHEMA income column; API.md income **Tax-deferred amount** paragraph, the new report section, and the 422 Response-codes entry; README Features line; the project note in `docs/ato/cgt-non-assessable-payments.md` updated to point at the field + report

## Inherited share parcels (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/inherited-assets-cost-base.md`, QC 66053.)

- [x] Mirror the s 115-30 discount-clock rule for inherited assets into `docs/ato/` (confirm from the ATO source: post-CGT asset → discount period runs from the deceased's acquisition; pre-CGT asset → from the date of death) and index it in `OVERVIEW.md` — read before implementing (`docs/ato/inherited-assets-cgt-discount.md`, QC 69713 "How CGT applies to inherited assets" — the rule confirmed verbatim)
- [x] Entry path for an inherited parcel: listing, holding account, units, date of death, cost base (recording which rule produced it), deceased's acquisition date (post-CGT case), LPR expenditure dated when incurred; provenance visible (not a market Buy) (`inheritances` entity, migration 0005; PUT creates the `trades.inheritance_id`-linked Buy atomically)
- [x] The parcel flows through every report and write-time capacity check like a Buy; the discount clock follows the mirrored s 115-30 rule (the Buy carries `deemed_acquisition_date` = deceased's acquisition for post-CGT; pre-CGT runs from the death date itself)
- [x] Web UI via the existing config-driven entity/action patterns (`ENTITIES` entry with `typeField` = cost_base_rule field groups)
- [x] Tests: cost-base and discount-clock cases (post-CGT and pre-CGT deceased); `ato_examples.rs` acceptance test for any representable worked example (the QC 66053 Maria/Antonio LPR-expenditure example; its other two examples classify deductibility only — noted in the `ato_examples.rs` header)
- [x] Docs: `docs/SCHEMA.md`, `docs/API.md`, README Features; Known limitations: estate/LPR side not modelled, market value at death is user-supplied

## Renounceable rights — selling, lapsing, retail premiums (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/rights-issues.md` already documents the sold/lapsed treatment. Supersedes the README line saying selling/lapsing rights is not modelled — update that text as part of this work.)

- [x] Sell-rights operation against a `RightsIssue`: units (capped, together with exercises, at the entitlement), proceeds per right, sale date → provenance-marked disposal taking the **original parcel's acquisition date** for the discount; nil cost base for free rights, carried cost for paid rights (nil proceeds = lapse of a paid right → capital loss); reaches realised + net-capital-gain reports — `POST /corporate_actions/:id/sell_rights` writes a `rights_sales` row + `rights_sale_allocations` anchorings (migration 0006; its own tables, not a Sell trade, so the share holding is untouched — a Sell would consume parcels); the realised-gains report emits it as a `source = RightsSale` row (each allocation's discount clock runs from its parcel's possibly-deemed acquisition date) and the net-capital-gain buckets flow from there. The shared cap lives in `rights_exercise::db_rights_used` (exercises now count sales and vice versa) plus a per-parcel anchoring cap (a parcel anchors at most the rights its record-date units earned). Sales are immutable (delete = undo, freeing the entitlement); the action and the anchoring parcel Buys are frozen while referenced
- [x] NEEDS CLARIFICATION: retail premiums — fetch and mirror the ATO retail-premiums guidance into `docs/ato/`, resolve the income character, and only then model (or record out of scope) — resolved via `docs/ato/retail-premiums.md` (QC 21832, retrieved 2026-06-10): under a **renounceable** offer (this project's `RightsIssue`) a retail premium is a **capital gain, not a dividend** (TR 2017/4) — modelled as a rights sale with the premium as proceeds, covered by the operation's tests; under a **non-renounceable** offer it is an unfranked dividend (TR 2012/1) — out of scope for modelling, entered as unfranked dividend income (README/API Known limitations). The earlier `rights-issues.md` paragraph calling all retail premiums unfranked dividends was the pre-TR 2017/4 view and is corrected
- [x] Web UI: `ACTIONS` entry — `sell-rights` action on RightsIssue rows (allocation editor posting `units` anchorings) plus a delete-only Rights Sales list view (`web::tests::rights_sales_ui_present`, `corporate_actions_ui_present`, `post_actions_are_config_driven`)
- [x] Tests: entitlement cap shared with exercises (`rights_sale::tests::entitlement_cap_is_shared_with_exercises`); discount anchoring (`realised_gains::tests::pure_rights_sale_discount_anchors_to_the_original_parcel`, `db_rights_sale_flows_into_the_report`, `net_capital_gain::tests::db_rights_sale_gain_enters_the_year_buckets`; lapse-of-paid-right loss in `pure_lapsed_paid_rights_realise_the_carried_cost_as_a_loss`); `ato_examples.rs` Example 39 (`rights_issues_example_39_shanti_sale_of_rights` — the post-CGT $50 gain)
- [x] Docs: `docs/SCHEMA.md` (tables + Relationships), `docs/API.md` (operation + 422 cases + realised-gains `source` field + Response codes + Known limitations), README Features text superseded; `docs/ato/rights-issues.md` + `OVERVIEW.md` updated

## Takeovers with a cash component — partial scrip-for-scrip rollover (2026-06-10)

(REQUIREMENTS 2026-06-10; `docs/ato/takeovers-and-scrip-for-scrip.md` Example 27 (Gunther). Supersedes the README "partial cash consideration not modelled" note — update it as part of this work.)

- [x] Extend `ScripForScrip` with an optional per-unit cash component; the exchange operation apportions each consumed parcel's remaining reduced cost base between cash and scrip by the consideration's market values, recognises the cash-side gain/loss (discount per the original holding period) in the realised + net-capital-gain reports, and creates replacement parcels for the scrip side exactly as today — three new columns (`scrip_cash_per_unit` per old unit, `scrip_market_value` per new unit just after issue, `scrip_cash_currency`; migration 0007, all-or-none CHECKs, only on ScripForScrip rows). The exchange (`entities/scrip_exchange.rs`) apportions cash×old / (cash×old + mv×new) of each parcel's reduced cost base to the cash side — kept as a numerator/denominator pair so exact fractions (Gunther's 1/3) divide once — prices the closing Sell at the cash per old unit in the cash currency, and carries only the scrip side's share into the replacement Buys. The realised-gains report now *includes* a scrip closing Sell whose action has cash (LEFT JOIN carries the terms in; `SellInfo::scrip_cash_apportionment` scales each allocation's pipeline cost base), with the discount classified by the original parcel's (possibly deemed) acquisition date; net-capital-gain inherits via `db_realised_gains`. The performance report counts the cash leg as real external proceeds (per-holding and OVERALL) on top of the carried cost
- [x] All-scrip behaviour unchanged; pure-cash takeovers remain ordinary Sells — all-scrip exchanges still write a zero-proceeds Sell excluded from the CGT reports (the pre-existing scrip tests pass unchanged, e.g. `realised_gains::tests::db_scrip_exchange_closing_sell_is_excluded`); a pure-cash takeover stays an ordinary Sell entered via `PUT /sells/:id` (no action recorded), per the docs
- [x] Tests: apportionment arithmetic; `ato_examples.rs` Example 27 acceptance test — `scrip_exchange::tests::cash_component_apportions_the_cost_base_and_prices_the_closing_sell` + `cash_apportionment_scales_the_scrip_side_by_the_exchange_ratio`; `realised_gains::tests::db_partial_rollover_cash_component_realises_the_cash_side` + `pure_scrip_cash_apportionment_scales_the_cost_base`; `performance::tests::db_scrip_exchange_cash_component_counts_as_external_cash`; `corporate_action::tests::api_scrip_for_scrip_cash_component_round_trip` + `api_invalid_scrip_cash_payloads_return_422` + the CHECK cases in `db_check_rejects_mixed_payloads`; `ato_examples::takeovers_example_27_gunther_partial_scrip_for_scrip_rollover` (the $700 gain, the $300/$600 apportionment, the $6 Regal cost base, the FY2025 $350 net capital gain)
- [x] Web UI: the action/operation config gains the cash field — the three fields join the corporate-action form's ScripForScrip group (`config.js` fields/`fieldGroups`/`typeDescs`/columns), and the Exchange action page describes the cash split when the action carries one (`web::tests::corporate_actions_ui_present` asserts the fields ship in the bundle)
- [x] Docs: `docs/SCHEMA.md` (new columns + CHECKs + Relationships), `docs/API.md` (action payload, 422 cases, exchange semantics, Response codes), README Features (cash component now modelled; no-rollover and multi-class cases stay out), `docs/ato/takeovers-and-scrip-for-scrip.md` "How this project models it" rewritten for both shapes

## Known-limitation documentation — gifts, pre-CGT holdings, indexation (2026-06-10)

(REQUIREMENTS 2026-06-10. Documentation-only; no modelling.)

- [x] Known limitations (docs/API.md + README): gifts / off-market related-party transfers are a disposal at market value (market-value substitution) — enterable today as a manual Sell or Buy at market value — `docs/API.md` Known-limitations bullet (giver's proceeds and recipient's first-element cost base are both the market value at the time of the gift; gift out = manual Sell at market-value proceeds, gift in = manual Buy at market-value cost) + README scope-cuts paragraph; ATO guidance mirrored as `docs/ato/capital-proceeds-market-value-substitution.md` (QC 66021, retrieved 2026-06-11, Martha & Stephen gifting example) and indexed in `docs/ato/OVERVIEW.md`; pinned by `doc_checks::known_limitations_document_gifts_at_market_value`
- [x] Known limitations: pre-CGT holdings (acquired before 20 September 1985) are outside CGT and not modelled — the system would wrongly compute gains on such a parcel — `docs/API.md` bullet (no pre-CGT flag; should not be entered; notes the one modelled pre-CGT interaction — an inherited parcel pre-CGT in the deceased's hands gets the market-value-at-death cost base and is post-CGT for the beneficiary) + README paragraph; pinned by `doc_checks::known_limitations_document_pre_cgt_holdings`
- [x] Known limitations: the indexation method (pre-21 September 1999 acquisitions, frozen at Sep 1999) is not modelled; the 50% discount is used throughout — `docs/API.md` bullet (cross-referencing the existing inherited-parcels note) + README paragraph; ATO guidance mirrored as `docs/ato/indexing-the-cost-base.md` (QC 66024, retrieved 2026-06-11, Val worked example, the 68.7 frozen-CPI steps) and indexed in `docs/ato/OVERVIEW.md`; pinned by `doc_checks::known_limitations_document_indexation_method`

Section note: documentation-only sections are still test-pinned (CLAUDE.md: an item is done only with a passing test) — the new `#[cfg(test)]`-only `src/doc_checks.rs` module asserts each Known-limitations entry (and its mirrored ATO doc's QC header) stays present in `docs/API.md` / README via `include_str!`.

## AMIT cash distributions — assessable-income double-count (2026-06-12)

(REQUIREMENTS 2026-06-12, from the full-archive data-entry verification: AMIT cash income rows are needed to drive DRP but their cash inflates `dividends_assessable` ~$12k–22k/FY alongside the AMMA attribution — for an AMIT the AMMA is the only assessable record.)

- [x] AMIT distribution income rows are cash-only: they fund DRP reinvestment (per-share cross-check, ex-date enrolment check, residual chain unchanged) but contribute nothing to the tax summary's income lines (`dividends_assessable`, `gross_assessable_investment_income`, credits, withholding) — exclusion driven by the listing's `amit` flag or an explicit income kind, validated at write time — done via the listing's existing `amit` flag: the tax summary's income query joins `listings` and skips AMIT rows whole-row (`reports::tax_summary`), and the shared franking candidate/threshold loader excludes them the same way (`reports::franking::db_franked_dividends`), so legacy rows with components can neither be claimed nor count toward the A$5,000 threshold. The DRP chain is untouched (reinvestable cash, per-share cross-check, ex-date enrolment check, residual handling read the cash row as before). Write-time validation in `entities::income::db_upsert` keeps the rows cash-only: an AMIT listing's row must be `trust_income`, non-zero `franking_credits`/`lic_capital_gain_deduction`/`conduit_foreign_income` and any `tax_deferred_amount` are rejected `422` (the AMMA statement is the assessable record; the cash components and the source-withholding fields `foreign_tax_paid`/`tfn_withholding_tax` — which reduce DRP-reinvestable cash — stay enterable). Tests: `entities::income::tests::{db_amit_cash_only_row_accepted, db_amit_non_trust_row_rejected, db_amit_notional_components_rejected, db_amit_tax_deferred_amount_rejected, api_amit_franking_credits_return_422_with_detail}`, `reports::tax_summary::tests::{db_amit_cash_rows_excluded_from_every_income_line, db_legacy_amit_rows_with_components_fully_excluded, db_amit_credits_do_not_count_toward_franking_threshold}`
- [x] Non-blocking cross-check report (pattern: E4 cross-check): flag every FY with AMIT cash rows whose listing has no AMMA statement covering that year; an AMMA year with no cash rows is not flagged — `reports::amit_cash_cross_check` (`GET /reports/amit_cash_cross_check`): one alert per (AMIT listing, FY) with cash rows but no AMMA whose `tax_year_end_date` falls in that FY, carrying `cash_rows` and the AUD gross `cash_total_aud`; row FY attribution matches the tax summary (governing `entitlement_date` else `date_paid`); AMMA-without-cash years and non-AMIT listings never flagged. Tests: `reports::amit_cash_cross_check::tests::{db_cash_year_without_amma_is_flagged, db_amma_covering_the_year_clears_the_flag, db_amma_for_another_year_does_not_clear_the_flag, db_amma_on_another_listing_does_not_clear_the_flag, db_amma_without_cash_and_non_amit_listings_not_flagged, db_entitlement_date_governs_the_expected_fy, api_get_amit_cash_cross_check}`
- [x] Non-AMIT trust and ordinary dividend rows unchanged (regression tests) — `entities::income::tests::db_non_amit_rows_unaffected_by_amit_validation` (dividend with credits and trust row with tax-deferred amount still accepted) plus the whole pre-existing income / tax-summary / franking / E4 suite passing unmodified
- [x] Live-data check: with the 2020–2026 archive entered, each FY's `dividends_assessable` drops to the non-AMIT components only (PLS franked dividends), and the `amma_*` lines still reproduce the AMMA Part A figures — verified 2026-06-12 against a copy of the live DB: `dividends_assessable` is now FY2023 2,166.45 and FY2024 2,757.30 (exactly the PLS franked dividends) and zero in FY2021/22/25/26 (previously inflated A$9.5k–22k/FY by VDHG/HNDQ cash); every `amma_*` line, `franking_credits`, and `foreign_tax_offsets` unchanged. The cross-check flags exactly one row — VDHG FY2026, 3 cash rows, A$12,323.78 — correct, since the FY2026 AMMA isn't issued until after 30 June (the reminder is the report working as intended). Mechanism pinned by the unit tests above; the live run is not reproducible in-repo (the archive DB isn't committed)
- [x] Docs sync: `docs/API.md` Income + Tax summary sections, README Features, web UI config if the income form changes — API.md Income gains the "AMIT cash distributions (cash-only rows)" paragraph (rule + `422`s), Tax summary documents the whole-row exclusion, a new "AMIT cash cross-check" report section, and the Response-codes `422` row names the AMIT income rejections; README updates the AMIT/AMMA and Tax summary bullets and adds the AMIT cash cross-check bullet; web UI gains the REPORTS config entry (the income form itself is unchanged — no new fields) with `cash_total_aud` classified money and labelled "(AUD)". Pinned by `doc_checks::amit_cash_only_rows_documented` and `web::tests::amit_cash_cross_check_ui_present`

## Known-limitation documentation — RSU dividend equivalents, foreign broker interest (2026-06-12)

(REQUIREMENTS 2026-06-12. Documentation-only; no modelling. Doc-only items are test-pinned via `src/doc_checks.rs`.)

- [x] Known limitations: dividend equivalents on unvested RSU grants are ordinary income when paid and are not modelled — enterable manually as income if paid out in cash — documented as the "RSU dividend equivalents" Known-limitations entry (ordinary income when paid per TD 2017/26 — remuneration, s 6-5, not a dividend and not ESS discount; a cash payout is enterable manually as an income row), citing the new mirror `docs/ato/ess-dividend-equivalents.md` (TD 2017/26, retrieved 2026-06-12, indexed in `docs/ato/OVERVIEW.md`) and surfaced in the README scope-cuts paragraph. Pinned by `doc_checks::known_limitations_document_rsu_dividend_equivalents`
- [x] Known limitations: interest income reports at question 10 (10L) regardless of source; foreign broker-cash/money-market income strictly belongs at 20E — state the simplification — documented as the "Foreign broker-cash interest classification" Known-limitations entry (all interest at 10L; foreign broker cash / money-market sweep income strictly belongs at 20E per `docs/ato/tax-return-labels-2026.md`; the taxpayer reclassifies when lodging), cross-linked from the tax summary's Interest income paragraph and surfaced in the README scope-cuts paragraph. Pinned by `doc_checks::known_limitations_document_foreign_broker_interest_classification`

## FX conversion granularity — spot-rate override for one-off capital transactions (2026-06-12)

(REQUIREMENTS 2026-06-12: QC 18020 Examples 5/7 — an average rate is not a reasonable
approximation for a one-off purchase/sale of a large capital asset; today the monthly RBA rate is
compulsory because the per-trade `fx_rate` is fallback-only. Sources: `docs/ato/forex-average-rates.md`,
`docs/ato/forex-common-transactions.md`.)

- [x] A trade (Buy, DRP, Sell) can carry an explicit spot-rate override that wins over the
      imported monthly RBA rate everywhere the trade's amounts convert to AUD (cost base,
      proceeds, every report and the snapshot pipeline). Design-open: promote `fx_rate` via an
      explicit flag, or a separate column — but entry must be deliberate; the silent fallback
      semantics of existing `fx_rate` rows must not flip — resolved as a **separate nullable
      column** `trades.spot_fx_rate` (migration `0010_spot_fx_rate.sql`; existing `fx_rate` rows
      keep their fallback meaning untouched, and entering the override is always deliberate).
      The one precedence rule lives in `infra::fx`: the new `FxOverride` enum
      (`None`/`Fallback`/`Spot`) replaces the old `Option<Decimal>` override parameter on
      `resolve_rate`/`to_aud`/`FxRates`, and `pick_rate` arbitrates spot > monthly ATO rate >
      fallback > loud failure — every conversion path (cost-base pipeline `ParcelRow::fx_override`,
      realised/unrealised/portfolio/open-parcels/performance/net-capital-gain-E10 reports, and
      the snapshot pipeline through them) goes through it, so no caller can re-derive precedence.
      Write-time validation (`trade::validate_spot_fx_rate`, shared by `PUT /trades` and
      `PUT /sells`) rejects a non-positive rate or one on an AUD trade with 422; the
      scrip-for-scrip exchange, demerger, and transfer operations carry a consumed parcel's
      override onto its replacement Buys so the carried AUD cost base is unchanged. Tests:
      `infra::fx::tests::{spot_override_wins_over_ato_rate, spot_override_converts_when_no_ato_rate_exists,
      fx_rates_spot_override_wins_over_ato_rate, from_trade_maps_spot_over_fallback}`,
      `trade::tests::{db_spot_fx_rate_round_trips_with_precision, db_spot_fx_rate_on_aud_trade_is_refused,
      db_non_positive_spot_fx_rate_is_refused, api_put_trade_with_spot_fx_rate_persists_and_aud_is_422}`,
      `sell::tests::api_sell_spot_fx_rate_persists_and_aud_is_422`,
      `open_parcels::tests::db_spot_fx_rate_wins_over_monthly_rate`,
      `realised_gains::tests::pure_spot_override_wins_over_preloaded_rates`,
      `scrip_exchange::tests::exchange_carries_spot_fx_rate_onto_replacement`,
      `demerger::tests::demerge_carries_spot_fx_rate_onto_replacements`,
      `transfer::tests::transfer_moves_parcel_preserving_cost_base_and_acquisition_date`, and the
      end-to-end ATO worked example `ato_examples::forex_example_lisa_via_spot_rate_overrides`
      (Lisa's figures reproduced with deliberately conflicting monthly rates imported)
- [x] Absent an override, behaviour is unchanged: monthly RBA rate first, `fx_rate` fallback,
      loud failure when neither exists (all pre-existing FX tests pass unmodified) — `NULL`
      `spot_fx_rate` maps to `FxOverride::Fallback(fx_rate)` with the identical precedence; every
      pre-existing FX/report test passes with no behavioural change (signatures only:
      `None`→`FxOverride::None`, `Some(x)`→`FxOverride::Fallback(x)`), incl.
      `realised_gains::tests::pure_manual_override_fallback_when_no_ato_rate` and the
      monthly-rate Lisa example `ato_examples::forex_example_lisa_usd_share_cost_base_and_proceeds`
- [x] Docs sync: `docs/API.md` FX conversion section states the rule honestly (monthly = the
      ATO-published convenience default, reasonable for recurring/small amounts; a one-off large
      foreign disposal should carry the transaction-date spot rate per QC 18020); `docs/SCHEMA.md`
      for any new column/flag; README FX bullet; web UI trade/Sell forms expose the override —
      API.md FX conversion section rewritten as the four-step precedence list with the QC 18020
      honesty note, Trades/Sells sections document the field and its 422s, Response-codes 422 row
      extended; SCHEMA.md trades table gains the `spot_fx_rate` row; README AUD-conversion bullet
      states the override and the convenience-default framing; `docs/ato/OVERVIEW.md` "FX
      conversion granularity" finding marked resolved; web UI: `spot_fx_rate` field on the trades
      and Sell forms (optional, hint carries the QC 18020 guidance), trades-list column,
      rate-classified in `COLUMN_KINDS`. Pinned by `doc_checks::fx_spot_rate_override_documented`
      and `web::tests::spot_fx_rate_override_ui_present`

## Settlement-window forex on foreign-currency trades — CGT events K10/K11 (2026-06-12)

(REQUIREMENTS 2026-06-12: under the default forex 12-month rule the contract-to-settlement
currency movement adjusts the cost base on an acquisition and is a separate non-discountable
K10 gain / K11 capital loss on a disposal — QC 17062, Art Ltd and Eleanor examples; the system
computes neither. Source: `docs/ato/forex-cgt-12-month-rule.md`. NEEDS DECISION: model it, or
resolve out of scope as a Known limitation.)

- [x] Decide the scope explicitly: either model it — for a non-AUD trade, compute the forex
      movement between the trade-date and settlement-date translations of the consideration,
      folding it into the parcel's cost base on a Buy/DRP and surfacing it as a separate
      non-discountable K10 gain / K11 capital loss feeding the realised-gains and
      net-capital-gain reports on a Sell — or resolve it out of scope as a Known-limitations
      entry stating settlement-window forex outcomes are the taxpayer's manual adjustment
      (doc-only resolution is test-pinned via `src/doc_checks.rs`, citing
      `docs/ato/forex-cgt-12-month-rule.md`) — **decided 2026-06-12: out of scope as a Known
      limitation.** Modelling would need a second translation of the consideration at the
      settlement date plus a new non-discountable K10/K11 gain/loss line through the
      realised-gains and net-capital-gain reports, for a component that is nil by construction
      for every monthly-rate-entered trade settling inside its rate month (all of the live
      data); the omission is stated instead. New Known-limitations entry "Settlement-window
      forex on foreign-currency trades — CGT events K10/K11" in `docs/API.md` (the rule per
      QC 17062 with the Art Ltd/Eleanor examples, outcomes the taxpayer's manual adjustment),
      surfaced in the README scope-cuts paragraph, cross-referenced from
      `docs/ato/OVERVIEW.md` (index row + "FX conversion granularity" finding) and the
      `src/ato_examples.rs` module header. Pinned by
      `doc_checks::known_limitations_document_settlement_window_forex_k10_k11`
- [x] The resolution notes the interaction with the spot-rate override above: with monthly rates
      and a same-rate-month T+2 settlement the component is nil by construction; per-leg spot
      rates are what make it visible — stated in the Known-limitations entry (both halves) and
      asserted by the same doc_checks test ("nil by construction", "per-leg spot rates")

## AMMA capital-losses-applied double-count in the loss pool (2026-07-12 review, domain)

`gross_buckets` (`src/reports/net_capital_gain.rs:475-481`) adds each AMMA statement's
`capital_losses_applied` into the **taxpayer's own capital-loss pool** (`b.losses`), where it
offsets other gains and carries forward. Per the mirrored guidance
(`docs/ato/amma-statement-guidance-notes.md`, lines 82–89), the attributed CGT amounts on an AMMA
are **already reduced** for capital losses applied *at the trust level*, and the losses-applied
figure is a disclosure/disclaimer item — a trust cannot distribute capital losses to members, so
the investor must not apply them again. Counting them a second time inflates the loss pool and
understates `net_capital_gain` in any year an AMMA reports losses applied. The tax summary's
`amma_capital_losses_applied` CSV label `18 (working)` (`src/reports/tax_summary.rs:195`)
propagates the same reading.

- [x] Re-verify the treatment against the live ATO AMMA guidance (the trustee reporting notes and
      the Personal investors' guide gross-up worksheet: double the discounted gain, apply the
      *investor's own* losses, halve) and mirror anything newly relied on into `docs/ato/` —
      confirmed against the Personal investors guide to CGT 2025 Part C (QC 104651): Step 4
      applies only the investor's own current-year and carried-forward losses; mirrored as
      `docs/ato/personal-investors-guide-managed-fund-distributions.md` (indexed in OVERVIEW.md)
- [x] If confirmed: stop feeding `capital_losses_applied` into `GrossBuckets.losses`; keep the
      column stored and reported as informational (like `tax_free_amount`), fix the CSV label, and
      update `docs/API.md` / README where the netting order is described — `gross_buckets` no
      longer reads the column; the tax summary keeps its informational `amma_capital_losses_applied`
      line with CSV label `""`; `docs/API.md` (netting step 2 + label table), `docs/SCHEMA.md`
      (column note), and the struct/module docs all state the trust-level rationale
- [x] Adjust the existing `net_capital_gain` tests that assert the old treatment, and add a test
      pinning that an AMMA losses-applied figure does not offset unrelated realised gains —
      `db_amma_indexation_other_gains_and_losses` split into
      `db_amma_indexation_and_other_gains_are_non_discountable` +
      `db_amma_trust_level_losses_applied_never_enter_the_loss_pool` (a $1,000 losses-applied
      figure offsets neither the statement's own gains nor an unrelated realised gain, and does
      not carry forward); the tax-summary label-alignment test pins the `""` label. Bonus: the
      mirrored doc's worked examples are now representable, so `src/ato_examples.rs` gains
      `pig_managed_funds_example_26_bob_fund_gains_and_tax_deferred` (18H $303 / 18A $203 + the
      E4 cost-base half), `pig_managed_funds_example_27_ilena_own_loss_against_fund_gains`
      (own $100 loss → 18H $220 / 18A $60) and
      `pig_managed_funds_example_28_miriam_amit_cost_base_net_amount` (signed AMIT cost base net
      amount, both directions)

## Cost-base FX timing: AMIT/ROC reductions convert at the acquisition-month rate (2026-07-12 review, domain — decide)

`CostBase::into_aud_with` (`src/domain/cost_base.rs:204-239`) deliberately resolves **one** rate —
the parcel's acquisition month — and applies it to every component, including the AMIT
(`amit_reduction`) and return-of-capital (`roc_reduction`) reductions that happened in later,
possibly very different rate months. The translation rules (s 960-50; `docs/ato/
forex-common-transactions.md` translates each leg at its own transaction time) point at
translating each reduction at the rate of the period/payment it belongs to. The codebase is also
internally split: `g1_gains` converts a payment's *excess* at the **payment month**
(`src/reports/net_capital_gain.rs:389-390`) while the same payment's *reduction* inside the cost
base converts at the acquisition month.

In practice this only bites on non-AUD holdings with non-AUD ROC/AMMA reductions (none in the
live data — E10/G1 events are on AUD funds), so it may be acceptable to resolve out of scope.

- [x] Decide explicitly: convert each reduction at its own event/period month (extending the
      pipeline to carry per-event rates), or record the single-rate simplification as a Known
      limitation with the citation, noting the g1_gains asymmetry either way

Closed 2026-07-13. Decided: the single-rate simplification stays, recorded as a Known limitation
rather than extending the pipeline — per-event rates would restructure how AMIT reductions are
aggregated (a single cumulative scalar today) for a case that does not arise in the live data.
The `docs/API.md` Known-limitations entry states the rule, the s 960-50(6) citation (QC 18322,
`docs/ato/forex-common-transactions.md` — Lisa's per-leg translation), the g1_gains asymmetry
(the same payment's excess converts at the payment month, its reduction at the acquisition
month; the E10 excess uses the buy-month rate, consistent with the cost base), and when it
bites; the FX-conversion section cross-links it; the README scope-cuts line surfaces it; and
the `cost_base.rs` module doc (step 5) and `into_aud_with` doc comment state the deliberate
choice at the code. Pinned by `doc_checks::known_limitations_document_cost_base_fx_timing`.

## Foreign broker-cash interest reports at 20E (REQUIREMENTS 2026-07-13, known-limitations review)

Resolves the "Foreign broker-cash interest classification" Known limitation (2026-06-12):
`docs/ato/tax-return-labels-2026.md` puts interest-like income from a foreign payer at question
20 (20E assessable foreign source income, foreign tax withheld via the 20O FITO), not question
10 (10L) — previously the tax summary reported every interest row at 10L and told the taxpayer
to reclassify manually.

- [x] `interest_income` rows carry the payer classification and foreign tax withheld: migration
      `0011_interest_income_foreign_source.sql` adds `foreign_source` (0/1, CHECK, default 0 so
      existing rows keep their Australian-source meaning) and `foreign_tax_paid` (never negative
      and foreign-source-only by CHECK). Write-time invariants reject a negative
      `foreign_tax_paid`, foreign tax on an Australian-source row, and a TFN amount on a
      foreign-source row, each 422 naming the correcting field
      (`interest_income::tests::{db_foreign_source_round_trips,
      api_negative_amounts_rejected_422, api_withholding_source_mismatch_rejected_422}`)
- [x] The tax summary routes a foreign-source row to the new `foreign_interest_income` line
      (label `20E + 20M`, never 10L), joins its foreign tax to `foreign_tax_offsets` under the
      A$1,000 FITO de-minimis, and counts both interest classifications in gross assessable
      investment income; AUD conversion by the month paid as before
      (`tax_summary::tests::{db_foreign_source_interest_reports_at_20e_with_fito,
      db_foreign_interest_tax_subject_to_fito_cap, db_non_aud_foreign_interest_converted_to_aud,
      db_ato_labels_align_with_their_columns, db_csv_header_carries_interest_column}`)
- [x] Web UI: the Interest Income form carries the foreign-source flag and foreign-tax field,
      and the new report column is money-classified (`web::tests::interest_income_ui_present`)
- [x] Docs: SCHEMA.md columns, API.md interest-income + tax-summary sections and the CSV label
      mapping, README feature line; the Known-limitations entry removed and its pin test
      replaced by `doc_checks::docs_document_foreign_interest_source_classification`

## Pre-CGT entry rejected at write time (REQUIREMENTS 2026-07-13, known-limitations review)

Hardens the "Pre-CGT holdings" Known limitation (2026-06-10) from documentation into an
enforced invariant: the entry said pre-CGT parcels "should not be entered" because every report
would wrongly compute a capital gain or loss on them — now they cannot be.

- [x] Any trade or Sell dated before 20 September 1985 is rejected 422 through the shared
      `trade::check_amounts` (`AmountsError::PreCgtDate`; `CGT_START` now lives in `trade` and
      is shared with the inheritance module). The first CGT day itself stays accepted
      (`trade::tests::api_pre_cgt_dated_trade_rejected_422`,
      `sell::tests::api_degenerate_sell_amounts_are_rejected_per_shape`)
- [x] An inheritance whose date of death is before 20 September 1985 is rejected 422 under
      either cost-base rule — the parcel would be pre-CGT in the *beneficiary's* hands
      (s 115-30 deems acquisition at the death at latest); checked before the per-rule
      acquisition checks so the rejection explains the actual rule
      (`inheritance::tests` DeathPreCgt cases)
- [x] Docs: the Known-limitations entry now records the write-time enforcement (pinned by
      `doc_checks::known_limitations_document_pre_cgt_holdings`), the Trades section states the
      new core-figure rule, the Inheritances section and the Response-codes 422 list carry the
      new rejections, and the README scope-cuts line says entry is rejected. TD 2000/10
      Examples 1–2 and bonus-shares Example 35 (whose facts include a pre-CGT parcel alongside a
      post-CGT one) enter that parcel with the first post-CGT date, 20 September 1985, as a
      stand-in — the quantity/cost-base re-basing under test is date-independent, and each test
      documents the substitution

## Wash-sale report excludes crypto transfer network-fee disposals (REQUIREMENTS 2026-07-15)
A transfer's network-fee disposal is an ordinary loss-realising Sell (no `transfer_id`, so the
gains reports count it — correctly), so the wash-sales report flags it whenever a Buy of the same
crypto lands inside the ±30-day window. TR 2008/1 is purposive: the fee disposal is compelled by
the transfer, timed by it, and the fee units are never re-acquired — no Part IVA fact pattern.
Symmetric with the report's existing Buy-side provenance exclusions.
- [x] `db_wash_sales` never treats a Sell referenced by `transfers.fee_sale_trade_id` as a wash-sale candidate; the fee disposal's loss still counts in realised-gains / net-capital-gain / performance, unchanged — a `HashSet` of `fee_sale_trade_id`s filters the loss-Sell candidates before matching; the loss rows themselves come from `db_realised_gains` untouched
- [x] Genuine Sells keep flagging: an ordinary loss Sell of the same listing near a re-buy still alerts (including crypto)
- [x] Tests: a fee-bearing transfer whose fee disposal realises a loss + a Buy of the listing inside the window → no alert; an ordinary loss Sell in the same window → alert; fee-Sell loss still present in the realised-gains report — `wash_sales::tests::db_transfer_fee_disposal_is_not_a_wash_sale_candidate` (one fixture covers all three: the fee disposal's $50 loss is asserted in realised gains, and the only alert pairs the ordinary $500 loss Sell with the re-buy)
- [x] Docs: the exclusion + TR 2008/1 rationale in `docs/ato/wash-sales.md` "How this maps to the project", the `reports/wash_sales.rs` module docs, and `docs/API.md`'s wash-sales section


## An AMIT cost-base adjustment over a split applies the statement's per-unit figure to the wrong units (SCENARIOS B-24)
(SCENARIOS.md section B verification pass, 2026-08-15. `amit_adjustments.quantity` is stored **in
the parcel's as-acquired units** (SCHEMA.md), while `amma_statements.cost_base_adjustment` is the
fund's per-unit figure — per unit as the statement year saw them. The reduction is
`quantity × cost_base_adjustment` (`amit_adjustment::db_cost_base_reductions*`), which multiplies
two different unit bases whenever a share split or bonus issue falls between the parcel's
acquisition and the statement's year end.)
- [x] Reproduced: Buy 100 @ $10 on 2023-08-01 (cost base $1,000); 2-for-1 `ShareSplit` 2024-01-15;
  AMMA FY2024 with `units_held: 200`, `cost_base_adjustment: "0.50"` — the fund reduced the cost
  base by 200 × $0.50 = **$100**. `POST /amma_statements/1/generate_adjustments` writes
  `quantity: 100` and the reports apply **$50**: `remaining_cost_base` 950.00, not 900.00
- [x] Same fixture with `cost_base_adjustment: "6.00"` — the statement's $1,200 on a $1,000 cost
  base should leave nil plus a **$200 CGT event E10 gain** in FY2024. Reported: cost base 400.00,
  `cgt_event_e10_gain` 0. A $600 cost-base overstatement and a gain never reported
- [x] Neither existing check catches it, because both are counted in units while the error is in
  money: generation answers `units_adjusted: 200` / `units_held: 200` / `difference: 0` (it
  re-bases the stored quantity for the reconciliation but not for the multiplication), and
  `/reports/amit_adjustment_cross_check` returns empty
- [x] The generation guard is narrower than it reads: `db_generate` refuses only when the covered
  parcels convert to the year-end basis by *different* ratios (`SplitAcrossParcels`). Parcels that
  are uniformly on a pre-split basis pass — the single-parcel case above, and every multi-parcel
  set acquired wholly before the split
- [x] No workaround exists at the right figures: hand-entering `quantity: 200` is refused (`the
  adjusted quantity exceeds the trade's quantity`, capped by `trades.quantity`), so the only way to
  land the correct reduction is to double the statement's per-unit figure — which then disagrees
  with the fund's document and with the row's own `units_held`
- [x] Decide the fix: re-base the stored as-acquired `quantity` into the statement year's basis
  before multiplying (the `units_adjusted` re-basing already computes exactly this factor, via
  `corporate_action::split_adjusted_quantity`), or store the quantity in the statement year's basis
  and re-base the other way for the capacity check. The first keeps `quantity`'s documented meaning
  and the `trades.quantity` cap intact
- [x] Tests: `entities::amit_adjustment` (the reduction over a split), `reports::net_capital_gain`
  (the E10 gain it suppresses), and `entities::amit_adjustment_generation` —
  `db_a_split_across_covered_parcels_is_refused` already builds this exact fixture and asserts only
  the quantities (`created[0].quantity == 100`, `units_adjusted == 200`); it needs the money
  assertion that would have caught this
- [x] Docs sync: `docs/SCHEMA.md` (`amit_adjustments.quantity`), `docs/API.md` (AMIT adjustments,
  Generating AMIT adjustments, and the `ShareSplit` bullet's "AMIT adjustment quantities remain
  expressed in the parcel's as-acquired units")

**Fixed 2026-08-15**, by the first route: the stored as-acquired `quantity` is re-based into the
statement year's basis before it meets the per-unit figure. The multiplication now lives in exactly
one place — `entities::amit_adjustment::reduction_for(quantity, cost_base_adjustment, splits,
acquired, tax_year_end_date)` — the AMIT counterpart of `RocEvent::per_unit_for`'s re-basing of a
return-of-capital payment, so no caller can pair the two figures on mismatched bases again. All
three former sites call it: `db_cost_base_reductions_up_to`, `db_cost_base_reduction_detail`, and
`reports::net_capital_gain`'s `e10_gains` walk. Both reduction readers now take the caller's
`&mut SqliteConnection` instead of a generic executor (they need the split events as a second
read); every caller already passed `&mut *conn` on its own transaction, so the single-snapshot rule
is unchanged.

The `SplitAcrossParcels` generation refusal is **removed**, not kept: it existed because "one
per-unit figure cannot scale two unit bases", which the per-parcel re-basing makes false. Parcels
either side of a split now generate normally, each stored in its own as-acquired units and costed
on the year-end basis. `db_a_split_across_covered_parcels_is_refused` became
`db_a_split_across_covered_parcels_is_costed_on_the_year_end_basis`, asserting the money ($100 and
$25 on the two parcels) alongside the quantities — the assertion whose absence let this through.
Verified all three new tests fail against the un-re-based multiplication before the fix.


## A parcel reduced by both an AMIT adjustment and a return of capital loses the excess over its cost base (SCENARIOS B-07, B-08)
(SCENARIOS.md section B verification pass, 2026-08-15. `reports::net_capital_gain`'s `e10_gains`
and `g1_gains` each walk their own reduction chain from the parcel's **full** initial cost base,
blind to the other — `g1_gains`' doc comment states the assumption outright: "Independent of the
AMIT E10 walk above: E10 applies to trust units, G1 to company shares, so the two reduction chains
never share a parcel in practice." Nothing enforces it: a `ReturnOfCapital` on a listing whose
`amit` flag is set is accepted `204`.)
- [x] Reproduced: AMIT listing, Buy 100 @ $10 (cost base $1,000); `ReturnOfCapital` $6/unit paid
  2024-09-01; AMMA FY2025 `cost_base_adjustment: "6.00"`. `/portfolio/open-parcels` is right —
  both $600 reductions reported, `remaining_cost_base` floored to 0 — but
  `/portfolio/net-capital-gain` shows FY2025 `cgt_event_e10_gain: 0`, `cgt_event_g1_gain: 0`,
  `net_capital_gain: 0`. The **$200 excess is never reported**
- [x] It is lost, not deferred: selling the parcel for $15/unit in FY2026 books a $1,500 gain
  against the nil cost base, so the year's grossed figure is $1,500 where the correct total across
  the two years is $1,700
- [x] The error is always an understatement, never the reverse: each walk reports
  `its own reductions − cost base` where the truth is `both reductions − cost base`, so the
  reported total can only be short (by the cost base, once, whenever both walks fire; by the whole
  excess when neither individually exceeds)
- [x] Order-independent, at least: entering the action before or after the AMMA statement gives
  identical figures both ways (checked)
- [x] Decide the fix, and its scope: walk one combined reduction chain per parcel in date order
  (the AMMA statement's `tax_year_end_date` against the payment's `date`), attributing each excess
  to the event that caused it — versus refusing the combination at write time (a `ReturnOfCapital`
  on an `amit` listing, which SCENARIOS E-04 asks about independently: an AMIT's cost-base movement
  is the AMMA `cost_base_adjustment`, and `PUT /income` already refuses a `tax_deferred_amount` on
  an AMIT row for exactly that reason). The refusal is much the smaller change and closes the case
  that arises in practice; the combined walk is what makes the reports correct for a fund that
  converts from a non-AMIT MIT mid-history (SCENARIOS F-23)
- [x] Tests: `reports::net_capital_gain` — a parcel carrying both reduction kinds, the excess
  reported once and in the right year; and the write-time refusal if that is the chosen route
- [x] Docs sync: `docs/API.md` net capital gain (the CGT event E10 and G1 paragraphs each describe
  their own walk in isolation) and, if refused at write time, the Income/Corporate actions sections

**Fixed 2026-08-15** by the combined walk, and the write-time refusal was **rejected** — not
merely deferred as the larger job. `income.tax_deferred_amount`'s own documentation settles it: a
non-AMIT trust's CGT event E4 cost-base reduction "is entered as a `ReturnOfCapital` corporate
action", the figure on the income row being informational and cross-checked only. So the AMIT +
return-of-capital combination is exactly how a fund that converts from a non-AMIT MIT to an AMIT
part-way through a holding must be recorded (SCENARIOS F-23), and `amit` is a listing-level flag
with no notion of *when* the conversion happened — a refusal keyed on it would reject the correct
entry for the earlier years while fixing nothing already recorded. SCENARIOS E-04's "is it
refused?" is therefore answered *no, deliberately*.

`e10_gains` and `g1_gains` are replaced by one `cost_base_excess_gains`, which reads both reduction
kinds, merges them per parcel into a `Reduction` enum, sorts by the date each arises (an AMMA
statement at its `tax_year_end_date`, a payment at its payment date; AMIT first on a tie — the same
tie-break `domain::cost_base::adjustment_detail` already itemises same-date rows in, so the two
presentations agree on which event exhausted the cost base) and walks **one** running balance down
from the parcel's initial cost. That mirrors the single balance `adjusted_cost_base` nets both
kinds against, which is why the open-parcels view was right all along. Each excess is still
attributed to the event that caused it and keeps that event's own conventions — E10 in the
statement's income year at the parcel's buy-month rate, G1 in the payment's income year at the
payment-month rate, pro-rated to the units still held at the payment — so the informational
`cgt_event_e10_gain`/`cgt_event_g1_gain` split is unchanged in meaning. The sort is stable and
keyed on the event date only, so entry order still cannot move a figure.

Four tests in `reports::net_capital_gain`: the reproduction above (both $600 reductions, the $200
excess now reported as E10 in FY2025, with the open-parcels figures asserted alongside to pin that
the cost base was always right); the same fixture entered in the reverse order; the FY2026 sale
showing the $1,700 two-year total rather than $1,500; and the mirror attribution — an AMMA
statement that stays within the cost base followed by a payment that overruns it, whose excess is a
**G1** gain in the payment's year.

## An AMIT adjustment covering part of a parcel is diluted across the whole parcel (SCENARIOS D-13)
(SCENARIOS.md section D verification pass, 2026-08-15. `amit_adjustment::db_cost_base_reductions`
computes each row's reduction as **covered quantity × the statement's per-unit figure**
(`reduction_for`, `src/entities/amit_adjustment.rs:190`) — the row's own arithmetic says the amount
belongs to those units — but `domain::cost_base::adjusted_cost_base` then subtracts it from the
*whole parcel's* initial cost and pro-rates the remainder over every as-acquired unit:
`(initial − amit) × units / parcel.quantity` (`src/domain/cost_base.rs:211`). While the row covers
the whole parcel the two agree exactly. They diverge the moment it covers less — which is not an
exotic hand entry: `amit_adjustment_generation` writes `quantity = parcel.remaining_as_of`, the
units still open at the statement's year end, so **every parcel partly sold during the year**
generates one.)
- [x] D-13 — reduction meant for the units still held is spread onto units already sold.
  Reproduced: Buy 2022-01-10 ×100 @ $10 (cost base 1000), Sell 2024-03-01 ×40, AMMA year ended
  2024-06-30 with `units_held: 60` and `cost_base_adjustment: 0.50`, one adjustment row covering 60
  units (reduction 30.00 — what generation itself would write). Realised gains report the 40 sold
  in March at cost base **388.00** (1000 − 30 pro-rated: (970 × 40/100)), and open parcels report
  the remaining 60 at `remaining_cost_base` **582.00** with `amit_cost_base_reduction: 30.00` —
  where 60 units each reduced by the stated $0.50 is 600 − 30 = **570.00**. 12.00 of reduction has
  moved from the units the statement covers to units it does not, understating the March sale's
  cost base and overstating what the open parcel carries into its own future disposal. The total is
  preserved, so the lifetime gain is unchanged — only its split across units and years is wrong
- [x] The AMIT adjustment cross-check does not see it (`units_adjusted: 60` equals the statement's
  `units_held: 60` — the set reconciles; it is the *application* of the reduction that doesn't), so
  nothing surfaces the figure
- [x] Contrast with the other cost-base reduction: a return of capital is applied **per unit** and
  bounded by `up_to` (`RocEvent::per_unit_for`, `src/entities/corporate_action/adjustments.rs:60`),
  so units sold before the payment are untouched and units held take the full per-unit amount. The
  two adjustment types answer the same question — which units does this reduction reach — in
  different ways, and only one of them matches the amount the row was computed from
- [x] Decide the model, which is the part that needs a call rather than code: a per-unit reduction
  applied to the covered units only (matching ROC, and matching `reduction_for`'s own multiplication)
  needs a rule for *which* units of a parcel a partial row covers — the units open at the year end
  is what generation means, but an entry covering the whole parcel after a mid-year disposal (the
  fund attributing to units held during the year, correct under s 104-107B: the adjustment is made
  "just before the end of the income year, **or just before the time of a relevant CGT event**",
  LCR 2015/11 para 13) must keep reaching the sold units, as it does today and as
  `reports::realised_gains::tests::db_amit_statement_for_the_sale_year_adjusts_the_parcel_already_sold`
  now pins
- [x] Tests: the partly-sold case above, asserting the sold allocation and the open remainder each
  carry the stated per-unit reduction and no more; plus the existing whole-parcel cases unchanged
- [x] Docs sync: `docs/API.md` AMIT adjustments (what `quantity` means for the units it does *not*
  cover) and, if the pooling stays, a Known-limitations entry saying so


**Fixed 2026-08-16** by making the AMIT reduction *per unit over the units the row covers*, the way
a return of capital already worked — the option chosen over documenting the pooling. The coverage
rule the fourth item asked for: a row covering the whole parcel reaches every unit (so the
already-pinned s 104-107B case is unchanged), and a row covering less covers the units still held
at the statement's `tax_year_end_date` **first**, spilling onto the units sold earlier only once it
covers more than those. Within each group the coverage spreads evenly, units of one parcel being
otherwise indistinguishable. The two rates always reconstruct `covered × per unit` exactly (the
single division comes last so the identity holds to the last decimal place), so coverage decides
the *split* and never the size — the reproduction's 30.00 is still 30.00, now 30.00 off the 60
units the statement names and nothing off the 40 already sold: sale cost base 400.00, remaining
570.00.

`db_cost_base_reductions`/`_up_to`/`_detail` are replaced by one
`db_cost_base_reduction_events(conn, up_to)` returning per-parcel `AmitReductionEvent`s carrying
the statement's per-unit figure, the covered units, and the units disposed of by its year end (read
via the shared `domain::open_parcels::db_units_sold`) — everything `reduction_for_units` needs.
`adjusted_cost_base`/`adjustment_detail` take those events instead of a pre-summed scalar, and
their `up_to: Option<NaiveDate>` becomes a `Held` enum (`AsAt(Option<date>)` / `DisposedOn(date)`):
the two cases answer `up_to()` identically, which is exactly why one `Option` could not tell held
units from sold ones, and so why the dilution was invisible. `CostBase::amit_reduction` — and with
it the open-parcels report's `amit_cost_base_reduction` — now means the reduction reaching the
*costed* units rather than the whole parcel; the adjusted figures it feeds are unchanged wherever a
row covers the whole parcel.

The net-capital-gain report's E10/G1 walk had to follow, or it would have measured an overrun
against a cost base the pipeline no longer uses: `cost_base_excess_gains` now walks **one chain per
group of units sharing an event history** (one per sale allocation, plus the units still held)
instead of one pooled whole-parcel chain. Where every reduction reaches every unit the groups'
chains are proportional and add back up to the pooled one — which is why every existing E10/G1 test
passed unchanged, including the combined-chain ones — and G1's old "notional whole parcel, then
scale the excess by held ÷ quantity" trick falls out of group membership instead of being spelled
out. It also drops the walk's duplicate `reduction_for` call: the reductions come from the same
loader the pipeline reads.

Tests: `domain::cost_base` gains the partial-row case both ways round (covered units reduced,
uncovered untouched), the whole-parcel contrast, the conservation identity across coverages,
spill-over beyond the units held, and the year-end boundary; `reports::realised_gains` pins the
reproduction end to end across the realised and open-parcels reports; `reports::net_capital_gain`
pins an E10 excess that only a per-group walk can see (a row covering 60 units at $12 exhausts
their $600 while the parcel's $1,000 would have absorbed it). The cross-check item needed no code:
`units_adjusted` reconciling to `units_held` was never the wrong figure — the application was, and
now matches. `docs/API.md`'s AMIT adjustments and open-parcels sections say which units a
`quantity` reaches and what `amit_cost_base_reduction` counts.

## A return of capital received on units already sold is not recorded anywhere (SCENARIOS D-14)
(SCENARIOS.md section D verification pass, 2026-08-15. Selling between a return of capital's record
date and its payment date leaves the seller entitled to the payment — that is what the record date
fixes — but the tool reduces nothing and records nothing, and correctly so as far as CGT event G1
goes: the units were not owned when the payment was made. The gap is what happens *instead*. The
ATO's own class rulings on returns of capital put it as CGT event **C2**, happening on the payment
date to the *right to receive* the payment, with a nil cost base for that right where the share's
cost base was fully applied in working out the gain or loss on the disposal — so the whole payment
is a capital gain in the payment's income year, not discountable (the right is held from the record
date). Nothing in the model can hold it.)
- [x] D-14 — reproduced: Buy 2023-01-10 ×100 @ $10, Sell 2023-10-03 ×100 @ $15 (gain 500.00), then
  a `ReturnOfCapital` of $0.50/unit dated 2023-11-01 with `record_date: 2023-09-25`. The sale's
  realised figures are unchanged (cost base 1000, gain 500) and the net-capital-gain report shows no
  `cgt_event_g1_gain` — right for G1, but the $50.00 actually received is nowhere: no capital gain,
  no income row, no cross-check flag. (A payment dated *before* the sale does reduce the sold
  parcel's cost base, back-dated entry included — that half is correct and pinned by
  `reports::realised_gains::tests::db_return_of_capital_needs_both_entitlement_and_holding_at_payment`)
- [x] `docs/API.md`'s `ReturnOfCapital` bullet states the two conditions precisely and says such
  parcels are left alone, which reads as *nothing to do* — the one place a user in this position
  would look. At minimum it should name the C2 event and the manual entry route; Known limitations
  has no entry for it either
- [x] Decide: document only (a Known-limitations entry plus the `ReturnOfCapital` bullet, with the
  entry route — there is no path that records a gain on a right, so it would be a manual note), or
  model it (the payment's units × per-unit as a C2 capital gain in the payment year for parcels
  entitled at the record date but disposed of before payment, which the existing record-date and
  allocation data is enough to derive)
- [x] Tests: `doc_checks` for the documentation route, or `reports::net_capital_gain` for the
  modelled one


**Fixed 2026-08-16** by **modelling** it, the option chosen over documenting it — the payment is real
money the return would otherwise never see, and the record date, allocations and sale dates already
in the model are enough to derive it.

Checking the ATO first changed the design. `docs/ato/cgt-non-assessable-payments.md` covers only
G1, so Class Ruling **CR 2025/59** (*Euroz Hartleys — return of capital*) is now mirrored as
`docs/ato/return-of-capital-right-to-receive.md` and indexed; the wording is boilerplate across the
return-of-capital rulings, so it is the general rule. It confirms the event, its timing and the nil
cost base as this section assumed — but **not** the discount treatment: para 18 puts G1 and C2 under
the *same* test, "you acquired your share at least 12 months before the Payment Date". The
discount is measured on the **share**, not on the right, so a C2 gain on a long-held parcel *is*
discountable. This section's "not discountable (the right is held from the record date)" was wrong,
and a hard-coded `false` would have overstated the tax on every C2 gain from a parcel held over a
year.

The report gains a `cgt_event_c2_gain` line beside the existing E10/G1 ones (JSON, CSV export with
its blank ATO label, the what-if scenario table, the annual tax report's summary, and
`COLUMN_KINDS` — which was also missing the E10/G1 columns, so all three now format as money).
`ExcessGain`/`ExcessKind` become `EventGain`/`CgtEventKind` and `cost_base_excess_gains` becomes
`non_disposal_gains`: a C2 gain is not an excess of anything, and what the three now have in common
is that they are capital gains with no disposal of the parcel behind them. The gain falls out of
the per-cohort walk D-13 introduced the day before, at no structural cost — the same question that
decides which units G1 reduces (was this group still held when the payment was made?) decides which
units C2 reaches instead, so the two are complementary by construction and a unit entitled at the
record date produces exactly one of them.

Four tests in `reports::net_capital_gain`: the reproduction (the $50 now reported in the payment's
FY2024, the sale's own $1,000/$500 untouched); the discount rule measured on the share's holding
period (the same fixture bought a year earlier: $550 discount-eligible, net $275 — 37 days and
fully assessable if measured from the record date); a parcel split across all three outcomes (30
sold before the record date and never entitled, 20 sold inside the window taking C2, 50 still held
taking the G1 reduction); and the no-record-date case, where entitlement falls back to the payment
date and nothing is reported until the record date is added. `docs/API.md`'s `ReturnOfCapital`
bullet and net-capital-gain section now name the C2 event, the nil cost base, the discount test and
the record-date requirement — so the one place a user in this position would look no longer reads
as *nothing to do*. No Known-limitations entry: it is modelled, and the record-date requirement is
stated where the field is.


## A return of capital in a currency other than its parcels' is accepted, then breaks every cost-base report (SCENARIOS E-07, E-39)
(SCENARIOS.md section E verification pass, 2026-08-16. `RocEvent::per_unit_for`
(`src/entities/corporate_action/adjustments.rs:74`) refuses — correctly — to net a payment against a
parcel in another currency, and raises `sqlx::Error::Decode`. Nothing checks the currency at
**write** time (`corporate_action::db_upsert`, `src/entities/corporate_action/db.rs:153`, validates
the payload's shape and the currency's existence, not its agreement with the listing's parcels), so
the mismatch is only discovered when a report reads it — as an `ApiError::Internal`, i.e. `500` with
an **empty body**.)
- [x] E-07 — reproduced: listing AAA (AUD), Buy ×100 @ $10, then
  `PUT /corporate_actions/1 {"action_type":"ReturnOfCapital","currency":"USD",…}` → `204`. From that
  moment `GET /portfolio/open-parcels`, `POST /portfolio/overview`, unrealised gains, realised gains,
  net capital gain, the annual tax report and snapshot generation all answer `500` with no body — the
  web UI can only show "HTTP 500". Nothing names the action, and no cross-check or health row points
  at it
- [x] E-39 — the same trap without a typo: exchange an AUD holding into a **USD-listed** replacement
  (a scrip-for-scrip replacement parcel deliberately keeps the *original's* currency, `docs/API.md`),
  then record the replacement listing's own return of capital in USD — its listed currency, the
  obvious entry — and every parcel report dies the same way
- [x] The precedent is B-02's brokerage-currency mismatch, refused at write time with a 422 naming
  the reason (`c7d7137`): the same fix shape applies here — compare the payment's currency against
  the currencies of the listing's Buy/DRP parcels inside the write transaction (and, symmetrically,
  refuse a Buy whose currency contradicts an existing payment on the listing, or the hole reopens
  from the other side)
- [x] `docs/API.md` currently documents the 500 ("A payment's `currency` must match the affected
  trades' currency — the reports never net amounts across currencies and fail loudly (`500`)"). If
  the write-time refusal lands, that sentence becomes the 422 instead

**Fixed 2026-08-16** by refusing the pair at write time, the fix shape the section proposed: a
return of capital reduces each parcel's cost base in the *parcel's own* currency, so a payment and
a parcel that disagree are now rejected from **either** entry side rather than discovered by a
report.

One shared read does both directions — `corporate_action::db_payment_currency_conflict`
(`adjustments.rs`, beside the `RocEvent::per_unit_for` guard it mirrors): the first
`ReturnOfCapital` on a listing whose currency differs from that of a Buy/DRP parcel it *reaches*,
run on the caller's own connection so each write checks the state it is about to commit.
Entitlement is the same test `per_unit_for` applies, expressed in SQL (acquired before the
`record_date`, or on/before the payment date with none recorded), so a parcel bought
ex-entitlement in another currency is no obstacle; the "still held" half is deliberately left out,
since a future sale can't be known at write time. Both callers run it *after* their INSERT, inside
the transaction, the way `allocations_fit_parcels` already checks written state:
`corporate_action::db_upsert` for a `ReturnOfCapital` write (`WriteError::PaymentCurrencyMismatch`)
and `trade::db_upsert` for a Buy/DRP (`UpsertError::PaymentCurrencyMismatch`). Both 422 bodies name
both currencies — the trade side names the payment's date too — so the disagreeing row is findable
without opening the other table.

Five tests: E-07's reproduction (the USD payment on an AUD holding now `422`, nothing persisted,
and `GET /portfolio/open-parcels` still `200` — the state that killed it is unwritable), E-39's
(an AUD parcel carried onto a USD listing, where the *listed* currency is the obvious wrong entry),
the entitlement scoping (a later USD parcel is no obstacle; with a record date the day either side
of it decides, and the refused edit leaves the stored terms untouched), the parcel-side twin in
`trade::tests`, and the docs pin.

`docs/API.md`'s sentence became the `422` as anticipated, plus the scope and the residual: the
scrip-for-scrip, demerger and transfer operations carry a parcel's own currency onto its
replacement without re-checking, so a replacement created *after* a differing payment is the one
remaining way to meet the mismatch — documented as still failing loudly. That matches B-02's own
scope (the operation paths bypass `check_amounts` too): the two hand-entry paths are where a
currency is typed. `docs/SCHEMA.md`'s `currency` column note now records the write-time validation
instead of the read-time failure.

## A return of capital on an AMIT listing double-reduces alongside the AMMA adjustment (SCENARIOS E-04)
(SCENARIOS.md section E verification pass, 2026-08-16. For an AMIT the cost-base movement is driven
solely by the AMMA statement's per-unit `cost_base_adjustment` — `docs/API.md` says so in the E4
cross-check section — but nothing stops the same money being entered *again* as a `ReturnOfCapital`
action on the same listing, and the two reductions simply add.)
- [x] E-04 — reproduced: AMIT listing VDHG, Buy ×100 @ $10, AMMA FY2024 with
  `cost_base_adjustment: 0.50` generated onto the parcel (`amit_cost_base_reduction: 50.00`,
  remaining cost base 950.00), then a `ReturnOfCapital` of $0.50/unit dated 2024-05-01 → `204`, and
  the parcel's remaining cost base drops to **900.00**. `e4_cross_check`, `amit_cash_cross_check`,
  `amit_adjustment_cross_check` and `health` are all empty: nothing sees it
- [x] **Decided 2026-08-16 (Evan): refuse it at write time** — a `ReturnOfCapital` on a listing with
  `amit = 1` answers `422` pointing at the AMMA statement's `cost_base_adjustment` as the place the
  reduction belongs. (The alternatives considered and rejected: a non-blocking cross-check row, or
  documenting it as the user's own call.) The refusal needs the usual sweep: the error variant and
  its 422 body beside `WriteError`, `docs/API.md`'s corporate-actions 422 catalogue, and a note in
  the AMIT/AMMA sections saying the two paths are mutually exclusive
- [x] Note the asymmetry that makes the refusal tempting: the income-row path already refuses the
  same double entry — `tax_deferred_amount` on a non-trust income row is a 422 telling the user to
  record a `ReturnOfCapital` instead — so the corporate-action side is the only unguarded door

**Fixed 2026-08-16 exactly as decided.** The income path turned out to refuse the *same* pair
already — `income::UpsertError::AmitTaxDeferred`, a `tax_deferred_amount` on an AMIT listing's row —
so the corporate-action refusal is that guard's mirror image, down to the 422's wording (it names
`cost_base_adjustment` and CGT event E10 as where the reduction belongs). That also settles the
"is a blanket `amit = 1` rule too coarse?" question the fund-converts-to-an-AMIT case raises: the
flag is already treated as a present-tense, unconditional bar on the E4 path elsewhere in the tree,
so a second, cleverer rule here would have been the inconsistency.
- [x] `corporate_action::db::WriteError::ReturnOfCapitalOnAmit` + its `From<WriteError> for ApiError`
      arm, checked inside `db_upsert`'s own transaction (before the INSERT — the listing's flag is
      all it takes, and an unknown listing still falls through to the FK violation as before). It
      runs over the state the write would leave, like its neighbours, so *moving* an accepted
      payment onto an AMIT listing is refused too; other action types on an AMIT are untouched
- [x] Tests: `entities::corporate_action::tests::api_return_of_capital_on_an_amit_listing_returns_422`
      (the E-04 reproduction — the payment is refused with a body naming `cost_base_adjustment` and
      E10, nothing persists, and the parcel keeps the AMMA statement's reduction alone: 950.00, not
      the 900.00 the accepted double entry produced) and
      `api_return_of_capital_is_refused_only_where_the_listing_is_an_amit` (the non-AMIT trust's E4
      payment still lands, the move onto an AMIT is refused and leaves the stored row alone, and a
      `ShareSplit` on the AMIT listing is unaffected)
- [x] `domain::open_parcels::tests::amit_and_return_of_capital_reduce_the_remaining_cost_base` now
      builds the one fixture that still reaches both reductions on one parcel — the payment recorded
      while the trust was not yet an AMIT, then the conversion — which is the case
      `docs/API.md`'s "one chain per parcel" paragraph already promised the cost-base chain handles.
      `entities::tests::what_a_get_returns_can_be_put_back_unchanged`'s corporate-action round trip
      moved to the fixture's non-AMIT listing
- [x] Docs: the corporate-actions write rules state the refusal and that the two paths are mutually
      exclusive (plus the converted-fund note: the refusal is on the write, so pre-conversion
      payments stand — record them before flagging the listing); the `ReturnOfCapital` bullet says
      it is the non-AMIT trust's E4 mechanism; the AMIT-adjustments section gained
      **This is an AMIT's only cost-base movement**, naming both shut doors from that side; the
      net-capital-gain "one chain per parcel" paragraph notes when the combination can still arise;
      and the Response-codes `422` row lists the refusal (alongside the E-07 currency mismatch,
      which was missing from it). Pinned by `doc_checks::amit_return_of_capital_refusal_documented`
- [x] Verified: `cargo build` and `cargo test` (1476 passed, warning-free), `cargo fmt --check` and
      `cargo clippy --all-targets -- -D warnings` clean

## Fractional entitlements are documented for splits and demergers but not for bonus issues or scrip exchanges (SCENARIOS E-11, E-36)
(SCENARIOS.md section E verification pass, 2026-08-16. The convention is consistent in the code —
exact fractional unit counts are kept everywhere, registry rounding and cash-in-lieu are never
modelled — but `docs/API.md` states it only for `ShareSplit` ("a consolidation that doesn't divide a
holding evenly keeps the exact fractional quantity") and `Demerger` ("registry cash-in-lieu of
fractional entitlements are not modelled").)
- [x] E-11 — a 1-for-10 bonus issue on 105 units reports **115.50** units held, where the registry
  issues 10 and pays cash for the half. The `BonusIssue` bullet lists only partly paid bonus shares
  and call payments as unmodelled
- [x] E-36 — a 1-for-3 exchange of 101 units creates a replacement parcel of
  **33.666666666666666666666666667** units (now pinned by a test). The `ScripForScrip` bullet lists
  multiple share classes, pre-CGT originals and loss rollovers as unmodelled, but not the fraction
- [x] Add the same sentence to both bullets, and say what to do with the cash actually received for
  a fraction (it is its own small CGT event on the disposed fraction — the honest answer may be
  "enter it as a Sell of the fractional units", which is worth stating rather than leaving to the
  reader)

**Resolution (2026-08-16): documented — the behaviour was already right and consistent; only the
convention and its consequence were unwritten.**

`docs/API.md` gained a *Fractional entitlements* section under Corporate actions, stating the
convention once for all four ratio-driven actions (`ShareSplit`, `BonusIssue`, `ScripForScrip`,
`Demerger`) with the worked figures from both findings — 10.5 bonus units on 105, and
33.666666666666666666666666667 replacement units on 101 — and why the exact figure is kept:
rounding would silently lose or invent part of a parcel with nothing recording the difference.
Both registry practices it declines to model are named (rounding the entitlement, and selling the
aggregated fractions for cash in lieu).

The question that left is answered rather than left to the reader: cash in lieu is the disposal of
the fraction and its own small CGT event, not a bookkeeping rounding — enter it as an ordinary Sell
of the fractional units dated the payment date, with the cash as its proceeds, so the fraction's
share of the cost base (and the discount, where the parcel qualifies) comes out of the same pipeline
as any other disposal and the holding is left at the whole-unit figure the registry holds. A
registry that rounds *up* instead has no CGT event to record. The `BonusIssue` and `ScripForScrip`
bullets now carry the sentence and link to the section; `ShareSplit`'s and `Demerger`'s existing
mentions link to it too, so all four say the same thing in one place. (The section header's stale
"Seven action types are modelled" was corrected to eight in the same pass.)

Tests: `doc_checks::fractional_entitlements_documented` (the convention, its reason, both worked
figures, the cash-in-lieu answer, and all four cross-links) and — E-11's behaviour, which unlike
E-36's had no pin —
`reports::open_parcels::tests::db_bonus_issue_keeps_the_exact_fractional_entitlement`
(105 units + 1-for-10 → 115.5, cost base unchanged). E-36 stays pinned by
`entities::scrip_exchange`'s existing fractional-replacement tests. Full suite 1481 passed / 0
failed.

## Which parcels an AMMA statement's per-unit figure reaches is undocumented (SCENARIOS F-05)
(SCENARIOS.md section F verification pass, 2026-08-16. Generation applies the statement's per-unit
`cost_base_adjustment` uniformly to every parcel open at the year end, and the Σ-against-`units_held`
reconciliation depends on it — but nothing in `docs/API.md` says so, and the ATO's own guidance
states the AMIT cost base net amount as a member-level annual amount without prescribing how it is
apportioned across parcels acquired at different times, `docs/ato/amit-cost-base-adjustments.md`.)
- [x] F-05 — a parcel bought 20 June, after the fund's final (31 March) distribution period, is
  covered at the same per-unit figure as a parcel held all year, and the registry's `units_held`
  at 30 June includes it, so the set reconciles. The year's total movement is therefore right while
  its split between parcels is an approximation — which matters when only some parcels are later
  sold. Behaviour pinned by
  `amit_adjustment_generation::db_a_parcel_bought_after_the_last_distribution_is_still_covered`
- [x] Document it in the [Generating AMIT adjustments](docs/API.md) section: the per-unit figure is
  applied to every unit held at the statement's year end, a member whose statement gives a *total*
  AMIT cost base net amount derives the per-unit figure by dividing over the units the statement
  covers, and a member who wants a different apportionment enters the rows by hand

**Resolution (2026-08-16): documented in [Generating AMIT adjustments](docs/API.md#generating-amit-adjustments).**

A "Which parcels the per-unit figure reaches" paragraph states the rule the reconciliation depends
on — `cost_base_adjustment` applies uniformly to every unit held at the statement's
`tax_year_end_date` — and then the two consequences a reader actually needs: a parcel bought after
the fund's last distribution period is covered at the same per-unit figure as one held all year
(the year's total movement is right; its split between parcels is an apportionment this tool makes,
not one the fund states), and a statement quoting a **total** AMIT cost base net amount is entered
by dividing it over the units the statement covers, which makes Σ reconcile by construction. The
way out is named too: a member wanting a different apportionment enters the rows by hand, where
`quantity` decides which units each row reaches. The paragraph cites
`docs/ato/amit-cost-base-adjustments.md`, which states the amount annually and per member without
prescribing how it is spread across parcels.

Tests: `doc_checks::the_per_unit_apportionment_across_parcels_is_documented` (the paragraph, both
consequences, and that the cited ATO mirror does state the amount as an annual member-level one);
the behaviour itself stays pinned by
`entities::amit_adjustment_generation::tests::db_a_parcel_bought_after_the_last_distribution_is_still_covered`
from the section-F pass. Full suite 1491 passed / 0 failed.

## An AMIT adjustment on a parcel closed by a transfer is accepted and reduces nothing (SCENARIOS F-17)
(SCENARIOS.md section F verification pass, 2026-08-16. A transfer closes the source parcel and
writes a replacement Buy carrying the cost base forward as a frozen figure
(`domain::rollover::insert_replacement_buy`, `src/domain/rollover.rs:255`) — so an AMIT adjustment
written against the *original* parcel afterwards reaches nothing: the parcel is fully consumed, so
no open-holdings report shows it, and the transfer's closing Sell is not a disposal, so no realised
gain nets it off. `amit_adjustment::db_upsert_on` checks the trade type, listing, holding account,
quantity and duplication — not whether the parcel still exists in any reachable form.)
- [x] F-17 — reproduced: Buy ×1000 @ $50 in account 1, transferred whole to account 2 on
  1 Feb 2025, then the sending account's FY2025 statement (0.20/unit) applied by hand to the
  original parcel (trade 10) → `204`. `GET /portfolio/open-parcels` still shows the replacement
  parcel at `amit_cost_base_reduction` 0 and `remaining_cost_base` 50,000; realised gains is empty;
  net capital gain is all zeroes. The $200 reduction is simply gone
- [x] The receiving account's own statement is fine — it covers the replacement parcel, which is
  the case pinned by
  `amit_adjustment_generation::db_a_parcel_transferred_mid_year_is_covered_in_its_new_account`
- [x] The same shape applies to any parcel-substituting operation (`domain::rollover` also backs
  scrip-for-scrip and demergers), and to any AMIT adjustment entered *after* one of them: the
  replacement's cost base was fixed when the operation ran
- [x] **Decided 2026-08-16 (Evan): option (a)** — refuse the adjustment, naming the replacement
  parcel. The options weighed were: (a) refuse an adjustment against a parcel that a rollover has
  closed, naming the replacement parcel to use instead (cheap, and makes the state
  unrepresentable); (b) carry a later adjustment through to the replacement parcel (correct in
  substance, but re-opens the "cost base frozen at operation time" decision the rollover design
  rests on); (c) flag it in the AMIT adjustment cross-check as an unreachable row

**Resolution (2026-08-16): option (a) — refused at write time, naming the replacement parcel and the
way round.**

`amit_adjustment::db_upsert_on` gained the check, as a natural widening of the existing
"quantity ≤ the trade's quantity" bound: the units a rollover has already carried away are
subtracted first, so a row may cover at most `trade.quantity − the units taken`. The three
parcel-substituting operations are exactly `domain::rollover`'s — transfer, scrip-for-scrip
exchange, demerger — recognised by the provenance column (`transfer_id` / `scrip_action_id` /
`demerger_action_id`) their closing Sell and their replacement Buys share. An ordinary Sell, a
buy-back participation and a worthless recognise are **real disposals** whose gain the reduction
does reach, so they are deliberately not counted: F-04's hand-entered whole-parcel row on a
sold-out year still writes, and a test pins that partition.

The refusal carries the units still adjustable (zero once the whole parcel went) and the
replacement parcel ids, and names the way round — *delete the operation, enter the adjustment, then
re-run it, so the replacement carries the reduced cost base forward*. That path is not advice taken
on trust: it is exercised by
`db_an_adjustment_entered_before_a_rollover_carries_into_the_replacement`, and walked end to end
against a running server on F-17's own reproduction (transfer deleted, adjustment entered,
transfer re-run → the replacement parcel carries 49,800 instead of 50,000).

One consequence is worth stating plainly rather than discovering later: an AMMA statement usually
arrives months after the year end, so a rollover can fall between the two, and **generation is then
refused as well** — the row-level check fires inside the generation transaction, writing nothing
partial. That is correct in substance (the reduction genuinely could not reach the replacement) and
the remedy is the same delete-enter-re-run, but it is a real workflow cost of option (a) over
option (b)'s carry-through. Pinned by
`amit_adjustment_generation::db_a_rollover_after_the_year_end_blocks_generation_with_the_reason`
and documented in the generation section.

Docs: the AMIT adjustments section states the refusal, its reason, the disposals it does *not*
reach, and the way round; the generation section states the same for a rollover after the year end;
the 422 catalogue row and the README's AMIT bullet follow.

Tests: `entities::amit_adjustment::tests::db_an_adjustment_on_a_parcel_a_rollover_closed_is_refused`
(F-17 exactly, with the replacement id), `db_a_partial_rollover_leaves_the_units_it_did_not_take_adjustable`
(the boundary is exact: 60 writes, 61 is refused), `db_an_adjustment_entered_before_a_rollover_carries_into_the_replacement`,
`db_a_parcel_closed_by_an_ordinary_sell_stays_adjustable`,
`api_rollover_replaced_parcel_returns_422_naming_the_replacement`, the generation test above, and
three `doc_checks` assertions. Full suite 1503 passed / 0 failed.

## The `amit` listing flag is retroactive and rewrites every earlier year (SCENARIOS F-23)
(SCENARIOS.md section F verification pass, 2026-08-16. `listings.amit` is a plain boolean with no
time dimension, and three readers key off it as though it had always been true: the tax summary
excludes *every* income row of an AMIT listing regardless of year (`src/reports/tax_summary.rs:352`
— `WHERE NOT l.amit`), the AMIT cash cross-check demands an AMMA statement for every year the
listing has cash rows (`src/reports/amit_cash_cross_check.rs:43`), and the `ReturnOfCapital` write
refuses the action outright once the flag is set (`src/entities/corporate_action/db.rs:337`,
E-04's fix). `docs/API.md` states conversion is supported — "a fund that converts to an AMIT
part-way through a holding keeps the payments recorded while it was an ordinary trust … record the
pre-conversion payments before flagging the listing" — which holds for the *cost base* but not for
the income side.)
- [x] F-23 — reproduced: an ordinary unit trust with an FY2023 distribution (franked 200,
  unfranked 300, franking credits 85). Tax summary FY2023: `dividends_assessable` 500,
  `franking_credits` 85. `PUT /listings/1` with `"amit": true` → `204`, and the tax summary is now
  **empty** — the whole pre-conversion year of assessable income vanished from the return, with no
  refusal, warning or health row. Only the AMIT cash cross-check notices, and it says the wrong
  thing: "FY2023 has cash rows with no covering AMMA statement", for a year in which there was no
  AMMA statement to have
- [x] The E-04 refusal is also broader than the documented advice: after the flip, *editing* an
  existing pre-conversion `ReturnOfCapital` (correcting the amount on a payment recorded years
  earlier) is refused `422` too, not only creating one. The stored reduction keeps applying, so
  the cost base is right until someone needs to correct it
- [x] **Decided 2026-08-16 (Evan): option (a)** — date the status on the listing. The options
  weighed were: (a) an `amit_from` date on the listing (or a
  small `listing_amit_periods` table), with every reader comparing the record's year against it;
  (b) drive the AMIT/non-AMIT decision off *which years have an AMMA statement* rather than off
  the flag, so the flag stays a UI hint; (c) declare mid-history conversion out of scope, document
  that a converted fund is entered as two listings, and have the write refuse the flag flip while
  the listing has income rows or a return of capital in an earlier year. Whichever is chosen, the
  income-side silence is the part that must not survive

**Resolution (2026-08-16): option (a) — `listings.amit_from`, compared by financial year in every
reader.**

Migration 0024 adds the nullable column (and re-creates the `listings` row_history trigger pair with
it, per the audited-table rule). NULL keeps the old, undated meaning — the flag covers the whole
history — so every existing row is already correct and nothing was migrated. Two write-time rules
pair it: `amit_from` needs `amit` (a date with no status to date means nothing), and it must be a
**1 July** date — AMIT status is *elected for an income year*
(`docs/ato/amit-reporting-requirements.md`), so it turns on at a year boundary; a mid-year date
would leave one financial year partly attributed and partly assessed as ordinary trust income.
Neither could be a table CHECK: SQLite cannot ALTER one in, and a column CHECK cannot reference
another column.

The comparison is stated once, in `entities::listing::amit_in_tax_year(amit, amit_from, tax_year)`,
and all five readers call it: the income entity's write-time checks (by the row's assessment year),
the tax summary's whole-row exclusion, the AMIT cash cross-check, the annual tax report's
`amma_missing`, and the `ReturnOfCapital` refusal (by the payment's own year). A converted fund can
no longer be an AMIT to one reader and an ordinary trust to another.

What that fixes, end to end against a running server: an FY2024 trust distribution (franked 200,
unfranked 300, credits 85, tax-deferred 100) entered while the fund was an ordinary trust **survives
the flag flip** — the tax summary still reports 500 assessable and 85 credits, where it previously
went to zero silently; the cash cross-check no longer demands an AMMA statement for that year; the
pre-conversion `ReturnOfCapital` is enterable *and* editable after the flip, which the E4
cross-check needs; and an AMIT-year payment is still refused. The upgrade path was exercised too, on
a database created before the migration: the column lands NULL, behaviour is unchanged, and the
rebuilt trigger records it.

Docs: the Listings section states the column, its 1 July rule and its reason, and names every reader
that compares against it; the Income, tax summary, cash cross-check, completeness, corporate-action
and net-capital-gain passages that stated the rule absolutely now state it per year; `SCHEMA.md`
carries the column and the trigger rebuild; the 422 catalogue and the README follow. The web UI's
listing form gains the dated field with a hint explaining when to use it.

Tests: `entities::listing::tests::db_amit_from_must_be_a_1_july_date_on_an_amit_listing` and
`amit_in_tax_year_turns_on_with_the_dated_financial_year` (the boundary: 1 July 2023 makes FY2024
the first AMIT year); `entities::income::tests::db_amit_checks_apply_only_from_the_conversion_year`
and `db_the_conversion_boundary_is_the_financial_year` (30 June is still ordinary, 1 July is not);
`reports::tax_summary::tests::db_a_converted_funds_pre_amit_years_are_still_reported`;
`reports::amit_cash_cross_check::tests::db_pre_conversion_years_are_not_asked_for_an_amma`;
`entities::corporate_action::tests::api_return_of_capital_on_a_converted_fund_follows_the_payments_year`;
`reports::tax_report::tests::amma_missing_ignores_years_before_the_fund_became_an_amit`; the 0024
trigger rebuild pinned in `reports::row_history::tests::audited_tables_match_migration_check_and_triggers`;
and `doc_checks::dated_amit_status_documented`. Full suite 1512 passed / 0 failed.

One incidental fix the migration forced out: `entities::listing_rename` read the listing with a
hand-spelled column list rather than `Listing::COLUMNS`, so the new column broke it at run time
(seven tests). It now uses the entity's own constant — the rule CLAUDE.md already states, and the
reason it states it.

## A franked dividend with no ex-date silently passes the holding-period test (SCENARIOS G-11, G-20)
(SCENARIOS.md section G verification pass, 2026-08-16. `Income::ex_or_pay_date` falls back to
`date_paid` when no `ex_date` was recorded, and the whole 45-day walk — entitlement snapshot and
qualification window alike — is anchored on that date.)
- [x] G-11 — 1,000 units bought 1 Jan, 400 sold 20 Jan (19 at-risk days), dividend ex 10 Jan and
  **paid 10 Feb** with $6,000 of credits attached: with `ex_date` recorded the walk denies $2,400,
  as it should. With `ex_date` left blank the same facts deny **nothing** — the walk snapshots
  entitlement at 10 Feb, by which time the 400 units are gone, so they are never entitled and never
  disqualified. The credits are claimed in full and `GET /reports/franking_at_risk` is empty
- [x] The fallback only works when the disposal is *after* the payment date (the shape
  `tax_summary::tests::db_missing_ex_date_falls_back_to_date_paid` pins). A disposal in the
  ex-date-to-payment window — the exact window the rule exists to catch — is invisible
- [x] G-20 — the common case is a trust distribution: units bought 1 June, **entitlement date
  30 June**, units sold 5 July, paid 20 July, $6,000 of credits. 33 days at risk, so the credits
  fail the rule; the system claims all of them. `entitlement_date` is deliberately not the franking
  anchor (`docs/API.md` Income, REQUIREMENTS 2026-06-xx) and no ex-date is printed on most trust
  statements, so nothing anchors the walk
- [x] **Needs a decision.** Options, not exclusive: (a) reject a row with attached
  `franking_credits` and no `ex_date` (`422` — the strongest, but every historical row was entered
  without one); (b) for a trust row fall back to `entitlement_date` before `date_paid` (a distribution
  goes ex at the period end, so this is the *right* proxy, and it fixes G-20 but not G-11);
  (c) surface it — a `franking_at_risk` row or health warning "credits attached, no ex-date recorded:
  the holding-period test could not be applied", which fails safe by naming what wasn't tested
- [x] Tests: the G-11 shape denies $2,400 whether or not the ex-date is recorded (or is flagged as
  untestable), and the G-20 trust shape reaches the same answer as the same facts with an ex-date

- [x] **Decided 2026-08-17 (Evan): options (b) and (c)** — anchor a trust row on its
  `entitlement_date`, and surface the dividends that still cannot be anchored. Rejecting the write
  (option a) was considered and rejected: every historical row was entered without an ex date

**Resolution (2026-08-17): the entitlement anchor is a chain, and what it cannot answer is
reported.**

`Income::ex_or_pay_date` now resolves the date the entitlement was fixed as `ex_date` →
`entitlement_date` (trust rows only) → `date_paid`, instead of skipping the middle step. That is
the *right* proxy rather than a convenient one: units go ex at the end of the distribution period,
and `entitlement_date` is that period's end — which is why a trust statement prints it and no ex
date. Both readers of the date follow, because both ask the same question (who was entitled):
the franking holding-period walk (`reports::franking`) and the DRP participation check
(`entities::drp_reinvestment`, where a distribution entitled inside an enrolment period but paid
after the unenrolment now correctly reinvests).

The G-20 shape is what changes: units bought 1 June, entitled 30 June, sold 5 July, paid 20 July,
$6,000 of credits. Anchored on payment, the walk found nothing held at 20 July, so the units were
never entitled and nothing was denied — the credits were claimed in full. Anchored on 30 June it
denies all $6,000, the same answer the facts give with an ex date recorded.

The residual case is a dividend with neither date (G-11): the walk still falls back to the payment
date, which cannot see a disposal made before it. `Income::ex_date_recorded` marks those rows, and
`GET /reports/franking_at_risk` now lists each one as `status: "untested_no_ex_date"` — nothing
denied, nothing at risk, just the dividend, its credits and the fact that the rule was never really
applied to it. Every row also carries `ex_date_recorded`, so a *denial* found on the fallback date
(which the tax summary does exclude) says the figure rests on it. That is what makes the report's
standing promise true: an empty report now means every attached credit is claimable. A buy-back's
dividend component is never untested — its `date_paid` is the tender date, which is exactly when
the entitlement was fixed (E-31), so nothing there is falling back.

Docs: the Income section states the chain and why anchoring a trust row on payment was wrong in one
direction only; the DRP reinvestment section resolves its ex date the same way; the franking at-risk
section lists the three statuses with `ex_date_recorded`; the README's foresight bullet says an
empty report really does mean every credit is claimable; the report's `desc` in `config.js` explains
the new status in the UI. No schema change — no new column, no migration.

Tests: `entities::income::tests::a_trust_rows_entitlement_date_anchors_the_entitlement_before_the_pay_date`
(the chain, including the buy-back exception),
`reports::franking_at_risk::tests::db_a_trust_rows_entitlement_date_anchors_the_holding_period_walk`
(G-20 end to end: the same $6,000 denied with and without the ex date recorded),
`db_a_dividend_with_no_ex_date_is_reported_as_untested` (G-11: the untested row, and recording the
ex date resolving it into a $2,400 denial), `db_a_denial_found_on_the_fallback_date_is_still_a_denial`,
`entities::drp_reinvestment::tests::a_trust_rows_entitlement_date_decides_participation_before_the_pay_date`,
`doc_checks::the_franking_windows_anchor_and_its_untested_rows_are_documented`, and
`web::tests::franking_at_risk_ui_present` (extended). Full suite 1522 passed / 0 failed.

## Conduit foreign income is excluded from assessable income with no stated entry convention (SCENARIOS G-03)
(SCENARIOS.md section G verification pass, 2026-08-16.)
- [x] G-03 — an `income` row with `unfranked_amount` 100 and `conduit_foreign_income` 40 reports
  `dividends_assessable` 100: the CFI figure is excluded from every total
  (`tax_summary::tests::db_conduit_foreign_income_excluded_from_assessable`, from the requirement
  "Exclude conduit foreign income from assessable totals")
- [x] That is correct **only** if the stored figure is a memo *within* `unfranked_amount`. For an
  Australian-resident individual — the report's stated `taxpayer_basis` — an unfranked dividend
  declared to be CFI is assessable: the ATO's AMMA guidance notes
  (`docs/ato/amma-statement-guidance-notes.md`, Part B item 13U) say to include it in "Dividends:
  unfranked amount declared to be CFI", "which forms part of the non-primary production income".
  CFI is NANE for *foreign* residents (Subdiv 802-A), which is not this system's taxpayer
- [x] Nothing states which reading the field takes: not `docs/API.md` (it appears only in the
  no-negative-amounts and AMIT-notional-component lists), not the field itself (the only `Income`
  field with no doc comment, against CLAUDE.md's every-field rule), not the UI (a bare "Conduit
  foreign income" input with no hint). A user who keys the statement's CFI line as its own amount
  understates the year's income silently
- [x] No report shows the figure either — the annual tax report's `dividends` rows carry franked,
  unfranked, credits, LIC and TFN, so a CFI-only row prints as a row of zeros
- [x] `docs/ato/OVERVIEW.md` attributes "conduit foreign income (NANE — excluded from assessable
  income)" to `mytax-managed-funds.md`, which contains no CFI text at all (nor does the live page) —
  the mirror does not support the claim the index makes for it
- [x] **Needs a decision**: document it as a memo subset of `unfranked_amount` (plus a write-time
  check that it does not exceed that amount, a line in the annual tax report, and an OVERVIEW /
  SCHEMA.md wording fix), or count it as assessable in its own right
- [x] Tests: whichever way it lands, a row carrying CFI reports the resident's assessable figure,
  and `doc_checks` pins the stated convention

**Decision (2026-08-17): the memo reading — `conduit_foreign_income` is the part of
`unfranked_amount` the payer declared to be CFI, recorded within it and never in addition to it.**

Counting it as an amount of its own was the alternative, and it is the worse of the two for a
reason that has nothing to do with which is more natural to key. The two readings are
indistinguishable in a stored row, so switching the totals to add the column would silently
double-count every row already entered the memo way — while the memo reading can be *enforced*,
turning the ambiguous row into a rejected write rather than a wrong number. It is also the reading
the source documents describe: the statement prints an unfranked amount with a CFI portion declared
out of it, and item 13U puts that portion in the non-primary production income. Nothing here is
excluded from assessable income; for the Australian resident this system reports for, the whole
unfranked amount is assessable and the CFI figure is counted exactly once, through it. (CFI is NANE
under Subdiv 802-A only for a foreign-resident member — the case this project does not model.)

So the report behaviour is unchanged and now correct *for a stated reason*: totals read
`unfranked_amount`, and `conduit_foreign_income` is read for reference only. What changed is
everything that let the wrong entry through unnoticed:

- **Write-time ceiling.** `income::db_upsert` rejects `conduit_foreign_income > unfranked_amount`
  with `422` (`UpsertError::ConduitExceedsUnfranked`, carrying both figures). That is precisely the
  data-entry error the old silence invited — the CFI line keyed alone, or beside a short unfranked
  amount — and the body says which way round the two go rather than only that the write failed. It
  runs after the AMIT checks deliberately: an AMIT row must carry no CFI at all, and that rejection
  names the better reason (the AMMA statement is the tax record). No schema CHECK and no migration:
  existing rows are left to the same rule on their next write.
- **The field says what it is.** `Income::conduit_foreign_income` carries the doc comment it never
  had (CLAUDE.md's every-field rule), stating the memo convention, the resident-vs-foreign-resident
  distinction, its ATO source, and how a split statement is entered (unfranked = the sum of the CFI
  and non-CFI lines).
- **The annual tax report prints it.** A `conduit_foreign_income_aud` memo column on both the
  dividend and trust-income tables, converted like every other figure, headed "CFI, within
  unfranked (AUD)" so the two columns can't be read as additive, with a note under the dividend
  table when the year actually has one. `docs/API.md`'s "every AUD figure sums to the matching tax
  summary line" promise now names this as its one deliberate exception — a memo has no line to sum
  to. The UI form carries the same convention as a field hint.
- **Docs.** A dedicated Income paragraph in `docs/API.md` (plus the new 422 in the response-code
  catalogue), the rewritten `SCHEMA.md` column note, and the tax-summary wording that used to call
  the exclusion "NANE" — it is not NANE here, it is already counted. `docs/ato/OVERVIEW.md`'s
  mis-attribution is corrected: the CFI claim is credited to `amma-statement-guidance-notes.md`
  (Part B item 13U), which does support it, and the `mytax-managed-funds.md` row now says it carries
  no CFI text — with a test that fails if that ever stops being true.

Tests: `entities::income::tests::db_conduit_foreign_income_above_the_unfranked_amount_rejected`
(the CFI line keyed alone, keyed beside a short unfranked amount, a proper subset, and the
wholly-CFI boundary) and `api_conduit_foreign_income_above_unfranked_returns_422_with_detail`;
`reports::tax_summary::tests::db_conduit_foreign_income_is_assessable_within_the_unfranked_amount`
(the resident's assessable figure is the whole unfranked amount — not netted, not doubled — and the
memo stays out of the foreign total) with `db_full_year_mixed_income_types` re-keyed to the
convention; `reports::tax_report::tests::conduit_foreign_income_prints_as_a_memo_column_and_is_not_double_counted`;
`doc_checks::conduit_foreign_income_entry_convention_documented`; and
`web::tests::income_conduit_foreign_income_memo_ui_present`. Full suite 1527 passed / 0 failed.

## A franking credit is accepted with no dividend behind it (SCENARIOS G-25)
(SCENARIOS.md section G verification pass, 2026-08-16.)
- [x] G-25 — `PUT /income/1` with `franking_credits` 300 and every other amount zero returns `204`,
  and the tax summary reports a $300 offset against $0 of dividend income
- [x] The same write accepts a credit ten times the dividend ($700 franked, $7,000 credits), which
  is arithmetically impossible: a company can attach at most `franked_amount × 30/70` (a base-rate
  entity's 25% gives less). It is the transposed-column / wrong-line data-entry error, and it
  inflates a *refundable* offset
- [x] Scope it to `trust_income = false` rows. A trust row's credit legitimately exceeds the ratio:
  the "franked distributions from trusts" component can be reduced by the trust's own deductions
  while the member still claims the full franking credit
  (`docs/ato/amma-statement-guidance-notes.md`, Part B item 13Q). AMIT rows already reject credits
  outright
- [x] **Needs a decision**: a write-time `422` naming the ceiling (the shape of the per-share
  cross-check and the no-negative-amounts rule), or a health-report warning (the shape of
  `duplicate_actions`)
- [x] Tests: a non-trust row with credits above `franked_amount × 30/70` is refused/flagged, a
  fully franked 30% row and a base-rate 25% row are both accepted, and a trust row above the ratio
  is left alone

**Decision (2026-08-17): a write-time `422`, not a health-report warning.**

A new ATO mirror settled it. `docs/ato/allocating-franking-credits.md` (QC 47305, fetched for this
item and indexed in `docs/ato/OVERVIEW.md`) gives the maximum franking credit as
`frankable distribution × (1 ÷ gross-up rate)`, the gross-up rate being
`(100% − corporate tax rate for imputation purposes) ÷ that rate` — **franked × 30/70** at the
standard 30% rate, less at every base-rate-entity rate (27.5%, 26%, 25%) and less again on a partly
franked distribution, so one ceiling covers every company distribution. More decisively, it settles
the *member's* side: where a statement shows a credit above the maximum, "the recipient is only
entitled to a franking credit equal to the maximum amount". The excess is not merely improbable — it
was never claimable. That is an arithmetic impossibility, not a judgement call, which is what
separates this from the duplicate-row findings (E-03, F-06, G-24): a duplicate is legitimate in
principle and so must stay enterable behind a warning, whereas an over-credited company dividend is
legitimate in no reading. The codebase already took the same view for the same figure on the other
entry path — a buy-back's terms have always rejected a franking credit with a zero dividend — and
CLAUDE.md's data-integrity rule puts invariants at write time.

`domain::franking_credit` is the shared rule (it must not diverge between the two writes that create
franked income), carrying the ATO citations:

- **Two rejections on non-trust rows**, both `422`: `FrankingCreditWithoutDividend` (a credit is
  attached to the franked part of a distribution — its message says where a trust's credits go
  instead) and `FrankingCreditAboveMaximum`, whose body names the ceiling and the transposed-column
  cause. Placed after the AMIT block in `db_upsert`, so an AMIT row still gets its own rejection.
- **Trust rows are exempt**, per Part B item 13Q — the reason the ratio genuinely need not hold
  there. A trust row with $900 of credits against $100 franked is left alone.
- **A rounding tolerance of the greater of one cent and 0.5%.** This was not a guess: a fixed cent
  rejects the ATO's *own* Example 6 (`you-and-your-shares-dividends.md`: $13,066 fully franked
  carrying $5,600, which an exact 30/70 puts 29 cents over, because the ATO rounded a $13,066.67
  dividend). Statements round both printed figures, so the tolerance is relative; it costs nothing
  in detection power, since the errors this catches are out by multiples.
- **Pre-1 July 2001 payments are out of scope**, when the corporate rate was 34%/36%/39% and the
  ceiling would be a wrong rejection. A scope cut, not an approximation — the check does not run.
- **The buy-back path is closed too.** `POST /corporate_actions/:id/participate` writes its dividend
  component with its own INSERT, bypassing `db_upsert`, so the invariant would not have held over
  the table. The check goes in the participation rather than on the action's per-unit terms, because
  only there are the figures the row will carry known: a per-unit ceiling can't carry a workable
  rounding tolerance (a cent is proportionally enormous against a per-unit figure, and scales up
  with the units).

Twenty existing franking fixtures set a credit with no franked amount — an impossible dividend they
never needed. `test_support::IncomeBuilder::fully_franked_credits(credits)` states such a fixture by
the credit at stake and derives the franked amount that carries it (credit × 70/30), which is what
those tests always meant; the rest were given explicit franked amounts.

Docs: an Income paragraph in `docs/API.md` with both scope limits and the trust exemption, the three
new 422s in the response-code catalogue, a form hint on the field, and the new ATO mirror indexed in
`docs/ato/OVERVIEW.md`.

Tests: `domain::franking_credit::tests` (the 30% maximum against the ATO's $700 → $300 and the
project's PLS figures, a base-rate 25% dividend under it, the ATO's own rounded Example 6 accepted,
the cent floor, the transposed pair reported with its ceiling, and the pre-2001 cut on both sides of
the boundary date); `entities::income::tests::{db_franking_credit_without_a_dividend_rejected,
db_franking_credit_above_the_company_maximum_rejected, db_a_trust_rows_franking_credits_are_not_capped,
api_franking_credit_above_the_maximum_returns_422_with_detail}`;
`entities::buyback_participation::tests::an_over_credited_buy_back_cannot_create_its_dividend_component`;
`doc_checks::franking_credit_ceiling_documented`; and
`web::tests::income_franking_credit_ceiling_hint_present`. Full suite 1540 passed / 0 failed.

## The related-payments rule and the 30%-at-risk test are not modelled and nowhere documented (SCENARIOS G-14)
(SCENARIOS.md section G verification pass, 2026-08-16.)
- [x] G-14 — being a "qualified person" needs more than the 45/90-day count: days on which 30% or
  less of the ordinary financial risk of loss and opportunity for gain is retained do not count
  (hedges, options, futures), and the **related payments rule** applies separately — the
  small-shareholder exemption itself only exempts a holder "entitled to franking credits for all
  shares that satisfy the related payments rule"
  (`docs/ato/you-and-your-shares-dividends.md`)
- [x] Neither is modelled (there is nowhere to record a hedge or a related payment) and neither is
  mentioned in `docs/API.md`'s Known limitations, the franking at-risk section, or the tax summary's
  `franking_credits` field — while that section states "an empty report means every attached credit
  is claimable", which claims more certainty than the recorded data can support
- [x] Documentation-only, like the C-09 rollover scope cut: state the two unmodelled tests, and
  qualify the empty-report sentence with them. Note that G-11's fix has since made that sentence
  *true for what the report does test* (a dividend the walk cannot anchor is now listed as
  `untested_no_ex_date`), so the qualification to add is about the tests that are not modelled at
  all, not about the walk's coverage
- [x] Tests: `doc_checks` pins the Known-limitations entry and the reworded report section

Closed 2026-08-17, documentation-only as scoped. A Known-limitations entry (**Franking: the
30%-at-risk test and the related payments rule**) states both tests in the ATO's own words, says that
neither the hedge/derivative position nor the related payment is recordable — so no stored fact could
test them — and names the trap that makes the second worth stating separately: the small-shareholder
exemption exempts a holder from the *holding period rule* only, so a related payment is not excused
by being under A$5,000 the way a short holding is.

The four places that reported on franking entitlement were qualified to say what their answer is
conditional on, rather than dropped to a hedge: the at-risk report's section in `docs/API.md` ("an
empty report means every attached credit is claimable **on the tests this report models**", still an
all-clear for those, since G-11's `untested_no_ex_date` means no dividend leaves the report
untested), the tax summary's `franking_credits` explainer, the README feature line (which made the
same unqualified claim — its `doc_checks` pin moved with the wording), and the report's own
description in the UI (`config.js`), so the screen doesn't promise more than the data can support.
`reports::franking_at_risk`'s module doc carries the same bound for the next reader of the walk.

Tests: `doc_checks::unmodelled_franking_qualified_person_tests_documented` (the limitations entry,
both tests named, the not-recordable facts, the exemption trap, the reworded report + tax-summary
sentences, and the cited ATO mirror still carrying both rules) and
`web::tests::franking_at_risk_ui_present` (the two unmodelled tests bound the description's
all-clear). Full suite 1546 passed / 0 failed.

## The LIC capital gain deduction field takes the already-halved figure, undocumented (SCENARIOS G-04)
(SCENARIOS.md section G verification pass, 2026-08-16.)
- [x] G-04 — `lic_capital_gain_deduction` is passed straight through to the tax summary's D8 line.
  What a LIC's dividend statement prints, though, is the **LIC capital gain amount (the attributable
  part)**; an individual deducts **50%** of it (`docs/ato/lic-capital-gain-deduction.md`: Ben's $50
  attributable part is a $25 deduction). The user must halve it before entering
- [x] Nothing says so: `docs/API.md`'s label table says only "The 50% LIC capital gain deduction is
  claimed at question D8", the UI field is a bare "LIC capital gain deduction", and there is no
  equivalent of the investment-expense field's explicit "enter the deductible figure
  (post-apportionment)" note. Entering the statement's figure doubles the deduction
- [x] **Needs a decision**: document the entry convention (a `docs/API.md` Income paragraph + a form
  hint naming the 50%), or take the attributable part and compute the 50% — which is what CLAUDE.md's
  "implement a requirement fully" argues for, but would need a migration and a re-reading of every
  existing row
- [x] Tests: the ATO example (Ben, $50 attributable part → $25 at D8) reproduced through whichever
  entry the decision picks

Decided 2026-08-17 (Evan): **take the attributable part and compute the 50%** — the second option, so
the statement's own figure is what gets entered and the doubling error mode disappears rather than
being documented around.

`income.lic_capital_gain_deduction` is now `income.lic_capital_gain_amount` (migration `0025`), and
`entities::income::Income::lic_capital_gain_deduction()` is *the* halving — the tax summary's D8 line
and the annual tax report's per-dividend `lic_capital_gain_deduction_aud` column both read it, so a
per-dividend figure can never disagree with the year's total (the report field names are unchanged:
they always were the deduction). The 50% is the individual rate the whole system assumes; the
33⅓% super/life rate stays out of scope under the existing *Taxpayer entity type* limitation.

Existing rows hold a deduction under the old convention, so 0025 reads them forward by doubling.
Money is TEXT decimal and must never round-trip through REAL, so the doubling is done on the
decimal's own digits as an integer — the digit string without its point, doubled, re-pointed at the
same scale — which is exact at every scale ('0.07' → '0.14', '1234567.895' → '2469135.790'); zero
rows (the column default, i.e. every non-LIC distribution) are skipped. `income` is audited, so the
migration drops and re-creates its two `row_history` triggers with the new column list (a
`RENAME COLUMN` would leave the JSON *key* saying `lic_capital_gain_deduction` while carrying the
attributable part). The doubling itself is deliberately un-audited — it runs while the triggers are
down, being a schema re-reading rather than an edit of a fact, and it inverts exactly (halve) if a
row turns out to have held the statement's amount already.

Docs: an Income paragraph in `docs/API.md` (enter as printed, with Ben's $50 → $25 worked through and
the pre-2026-08-17 convention noted), the tax-summary label-table row now naming the computed 50%,
`docs/SCHEMA.md`'s column, the README income-recording feature line, `docs/ato/OVERVIEW.md`'s row for
the cited mirror, and a form hint on the field warning that a pre-halved entry claims half the
deduction.

Tests: `ato_examples::lic_capital_gain_deduction_example_resident_individual` (Ben's $50 attributable
part entered through the API, $25 at D8), `reports::tax_summary::tests::db_lic_deduction_is_half_the_advised_amount`,
`reports::tax_report::tests::income_sections_sum_to_tax_year_summary` (the per-dividend LIC column is
the halved figure and sums to the D8 line), and
`infra::db::tests::migration_0025_doubles_the_lic_deduction_into_the_advised_amount` (the forward-read
applied to pre-0025 data at five scales, plus both row-history triggers back and naming the new
column). Full suite 1547 passed / 0 failed.

## Nothing states that interest belongs to the year it is *credited* (SCENARIOS H-05)
(SCENARIOS.md section H verification pass, 2026-08-17.)
- [x] H-05 — a term deposit credits $500 of interest on 30 June 2026; the funds are only reachable
  on 2 July. The ATO rule is the credit: "You must declare interest income in the year it is
  credited, received or applied or dealt with in any way on your behalf or as you direct … For term
  deposits this usually means you should declare interest in the year the investment matures"
  (*Investment income*, QC 72101, retrieved 2026-08-17)
- [x] The calculation is right — `tax_summary::tests::db_interest_is_assessed_in_the_year_it_is_credited`
  now pins that a 30 June `date_paid` lands in FY2026 and 2 July in FY2027. The gap is that
  `date_paid` is the *only* date the row carries, and nothing tells the user which of the two dates
  it wants: the field is labelled "Date paid" in the UI with the hint "Sets the financial year the
  interest falls in", `docs/SCHEMA.md` says "Date paid/credited", and there is no `docs/ato/` mirror
  of the rule at all. Key the availability date and a whole year's interest moves
- [x] The system already models this distinction where it decided to: a trust distribution carries
  `entitlement_date` beside `date_paid` precisely so a 30 June entitlement paid 15 July is assessed
  in the year just ended (G-19). Interest has one date because it needs one — but only if that date
  is named unambiguously
- [x] **Decided 2026-08-17: state the convention**, the shape the conduit-foreign-income convention
  took (G-03) — relabel toward "date credited" in the UI hint, `docs/SCHEMA.md` and `docs/API.md`,
  plus a mirrored `docs/ato/investment-income-timing.md` (QC 72101) indexed in OVERVIEW. No second
  column: the availability date is not a tax fact, and recording it would invite keying it here
- [x] Tests: `doc_checks` pins the stated convention against the mirrored ATO wording

**Implemented 2026-08-17 as decided: the convention is stated, nothing is modelled.**

`interest_income.date_paid` holds the date the interest was **credited** — credited, received, or
applied or dealt with on the holder's behalf or as they direct — which for a term deposit run to
maturity is the maturity date, and is never the date the funds became reachable. The calculation
did not change (it was already right); what changed is that every surface a user meets the field
through now says which date it wants:

- **The ATO mirror it rests on.** `docs/ato/investment-income-timing.md` (QC 72101, retrieved
  2026-08-17) carries the rule verbatim plus the three cases around it — a rollover or automatic
  reinvestment is a credit at the rollover date of the amount that would have been received, and a
  deposit paying interest away periodically is assessed at each of those payment dates — with a
  "why this matters" section naming the 30-June/2-July case and why interest gets one date where a
  trust distribution gets two. Indexed in `docs/ato/OVERVIEW.md` beside `trust-income-timing.md`,
  which it contrasts with.
- **The UI.** The field is labelled **"Date credited"** (was "Date paid") and its hint states the
  rule and the worked case, so the ambiguity is resolved at the point of entry rather than in a doc
  the user would have to go looking for.
- **Docs.** A dedicated paragraph in `docs/API.md`'s Interest income section — the quoted rule, its
  cite, the 30-June/2-July case, and the stated reason there is no second column (a trust
  distribution's `entitlement_date` exists because present entitlement and payment are two
  different tax facts; availability is not a tax fact at all, and a column for it would only invite
  keying it as the assessment date) — plus the rewritten `SCHEMA.md` column note and the
  tax-summary aggregation line, which now names `date_paid` as the credit date.

No second date column, no schema change, and no migration: the stored data was already right under
the convention, which is exactly why stating it was enough.

Tests: `doc_checks::interest_credited_date_convention_documented` (the mirror carries the ATO rule
with its QC header and is indexed; API.md and SCHEMA.md state the convention, the worked case, and
the no-second-column decision) and `web::tests::interest_income_date_credited_hint_present` (the
served bundle carries the relabelled field and its hint), over the existing
`reports::tax_summary::tests::db_interest_is_assessed_in_the_year_it_is_credited`.

## SCENARIOS V-d — a parcel dated before an already-run whole-holding operation is never consumed

Raised driving **V-03 / V-06** (a corporate action, and a back-dated acquisition, entered after
the facts they should have reached).

Three operations consume **every** open parcel of their listing as a matter of law, not choice:
the scrip-for-scrip **exchange**, the **demerge**, and the worthless-shares **recognise**. Each is
refused if the listing traded on or after its date, and `docs/API.md`'s *Recording one of the three
read-time events behind a rollover that has already run* refuses a `ReturnOfCapital`, `ShareSplit`
or `BonusIssue` dated on or before one — on the stated grounds that otherwise *"the same facts
entered in a different order would report a different cost base"*.

A **parcel** dated before one is not guarded. Measured, each accepted `204`:

| Operation (already executed) | Back-dated write | Result |
| --- | --- | --- |
| Exchange OLD → NEW 1-for-1, 2024-06-10 | Buy 50 OLD, 2024-02-05 | 50 units of a security that no longer exists; 50 NEW units missing |
| Exchange OLD → NEW 1-for-1, 2024-06-10 | Inheritance of 25 OLD, died 2024-03-01 | same, via the inheritance parcel path |
| Demerge HEAD 1-for-5, 2024-06-11 | Buy 50 HEAD, 2024-03-05 | no SPIN units issued; the parcel keeps 100% of its cost base instead of 90% |
| Recognise DEAD worthless, 2024-06-13 | Buy 40 DEAD, 2024-03-05 | 40 units still open on a company already written off |

Nothing surfaces any of them. `GET /reports/rollover_consistency` is blind by construction — it
compares what the **consumed** units are worth now against the replacements' stored figures, and
these units were never consumed — and the health report says nothing.

Not affected, and correctly so: a **transfer** and a **buy-back participation** move a *chosen*
quantity, so a parcel left behind is a legitimate outcome.

Options offered:

1. **Refuse at write time**: a parcel-creating write (Buy/DRP `PUT /trades`, inheritance, ESS
   vest, rights exercise) dated on or before an executed exchange/demerge/recognise on that
   listing answers `422` naming the operation and its date, with the recovery its sibling refusal
   already gives — delete the operation, enter the parcel, run it again.
2. **Report it**: extend `rollover_consistency` (and so the annual tax report's completeness
   section) with an *unconsumed parcel* problem naming every parcel open on the listing at the
   operation's date that the operation did not consume. Advisory; nothing is refused.
3. **Both** — refuse the write, and report any state that predates the guard, which is the pattern
   the AMIT-adjustment / rollover pair already follows.

**Evan chose option 3** — refuse *and* report: `422` at write time naming the operation and its
date, plus an *unconsumed parcel* problem on `rollover_consistency` for any state that predates
the guard.

- [x] Refuse a parcel-creating write dated on or before an executed exchange/demerge/recognise,
      with a test per affected operation and per parcel-creating path.
- [x] Add the unconsumed-parcel problem to `rollover_consistency` (and so the annual tax report's
      completeness section), with a test and the `docs/API.md` entry.

Done 2026-08-23. The shared rule lives in `domain::whole_holding`: the three operations' provenance
columns are named once (`CLOSING_SELL_COLUMNS`), so the guard and the report can never disagree
about what counts as a whole-holding operation, and the `422` body is built once
(`BackDatedParcel::message`) for all eight refusals. The comparison is on the trade's own `date`,
never `deemed_acquisition_date` — a rollover replacement and an inherited parcel both carry a deemed
date decades earlier for the discount clock alone. Paths covered, enumerated from every non-test
`INSERT INTO trades` in `src`: `PUT /trades/:id`, `PUT /inheritances/:id`, `POST
/ess_statements/:id/vest`, `POST /corporate_actions/:id/exercise`, `POST /income/:id/reinvest`, and
the replacement parcels of the scrip exchange (its *destination* listing), the demerger (its
*demerged* listing) and the transfer — the two rollovers' source/head listings are already covered
by their own "traded on or after" refusal. An **edit** of a parcel already sitting behind an
operation is deliberately not refused: a consumed source parcel is behind one by definition, and
that edit is the state the report exists to surface. `WorthlessShares` became a fourth
`rollover_consistency` `kind` rather than a report of its own — it is one of the three operations,
the row shape fits it exactly, and it reaches the annual tax report's completeness section for
free; it stores no carried figures, so only the unconsumed-parcel check runs on it.

## Multiply-before-divide in the three sites W-b and W-e left standing

Held open deliberately rather than archived with section W: all three are the *same* shape W-b fixed
in `domain::cost_base`, and none is reachable by the magnitude bound W-e added — because in each case
there **is** a lesser answer to give, which is exactly what separates them from an unrepresentable
cost base. Two were found while fixing section W; the third was found by grepping the shape rather
than trusting the two the section named, and was measured at the HTTP surface before being fixed.

- `AmitReductionEvent::reduction_for_units` — `per_unit * covered.min(held) * units / held`. Probed
  at 1e15 units with a `0.05` per-unit adjustment it survives (`1.5e28`); a larger per-unit figure at
  that scale overflows. **Two** multiplications precede the divide, and either can overflow: a
  per-unit figure of 1e15 overflows on the first one alone.
- `entities::demerger::db_demerge` — `carried_cost_base * cost_base_pct / Decimal::ONE_HUNDRED`, and
  `at_date_units * new_units / held_units`. A parcel costed at ~1e27 demerged at any percentage
  overflows the intermediate even though the result is representable. This one is in a **write**
  path, so it is the more serious of the two.
- `entities::investment_expense::check_apportionment` — `to_cents(gross * pct / Decimal::ONE_HUNDRED)`.
  A **write-time validation**, so the panic aborts a legitimate write: `gross_amount` is checked for
  sign but has no magnitude bound (W-e bounded parcel-creating writes and Sells, not this), so a
  gross of 1e27 at 100% answered a logged `500` while the same gross at 50% (`5e28`, under the
  ceiling) was accepted `204`. Both the gross and the answer are representable; only the working was
  not.

All three now answer a logged `500` rather than resetting the connection (W-b's panic layer), so they
fail safely; none answers correctly. The fix is `domain::cost_base::prorated_initial_cost`'s treatment —
`checked_mul` first so no figure that fits today moves by a digit, divide-first only on the overflow —
not a refusal.

- [x] Apply the `prorated_initial_cost` treatment to `AmitReductionEvent::reduction_for_units`, to
      `entities::demerger::db_demerge`'s two pro-rating expressions, and to
      `entities::investment_expense::check_apportionment`, with a test at each old ceiling
      — the treatment is now one shared helper, `infra::decimal::mul_div(&[factors], divisor)`:
      multiply left to right then divide (byte-identical to the expression it replaces, since `*`
      and `/` associate left to right), taking the division early only at the first product that
      overflows, and panicking as before where the *result* itself is unrepresentable. It lives in
      `infra::decimal` beside `to_cents` because it is an arithmetic primitive over `Decimal` with
      no domain meaning of its own — it knows nothing about cost bases — and because `entities/`,
      `domain/` and `reports/` all need to reach it. The slice of factors is what lets one helper
      serve both the four-term AMIT shape (`a × b × c / d`) and the three-term one everywhere else.
      `prorated_initial_cost` keeps only its `units == quantity` identity short-circuit and
      delegates the rest, so there is one implementation of the idea. The demerger's `held_units`
      divisor is validated positive at the corporate-action write path
      (`CorporateActionBody::kind`'s `positive`), confirmed rather than assumed. Tests:
      `infra::decimal::tests::{mul_div_is_the_plain_expression_wherever_that_fits,
      mul_div_multiplies_before_it_divides,
      mul_div_answers_where_the_product_overflows_but_the_result_does_not}`,
      `domain::cost_base::tests::{an_amit_row_past_the_old_multiply_first_ceiling_still_reduces,
      a_reduction_that_fits_keeps_its_multiply_first_figure}`,
      `entities::demerger::tests::api_demerge_past_the_old_multiply_first_ceiling_completes`,
      `entities::investment_expense::tests::api_apportionment_past_the_old_multiply_first_ceiling_reconciles`.
      Each fails with the fix removed; a divide-first implementation additionally fails eight
      pre-existing tests, which is what says the multiply-first order is load-bearing rather than
      incidental.

## Multiply-before-divide across the rest of the tree — sixteen more sites

Found by grepping the `a * b / c` shape while closing the three sites above (that grep is also what
turned up `investment_expense`, which the section above had not named and which was reachable). The
sixteen below are what the shape match returns; it is a *textual* match, so a near-variant it does
not spell (a dereferenced operand, a constant multiplier) can hide from it — two of these sixteen
were found only by reading around the hits. Every
one of these is the same shape and the same failure mode: an intermediate product past
`rust_decimal`'s ~7.9228e28 ceiling panics, which the panic layer turns into a logged `500` with an
empty body, even where the answer itself is perfectly representable. The helper the three fixed sites
now share, `infra::decimal::mul_div`, makes each a one-line substitution — the work is a test per
site and confirming each divisor is non-zero, not the edit.

Not fixed here deliberately: this is a separate decision, and a sweep is worth taking as one pass
with its own reproductions rather than trailing the three sites that were measured.

The ones a user-entered figure can actually drive — no magnitude bound stands between an entry and
the product, and W-e bounds a *cost base*, never a quantity:

- `entities::scrip_exchange.rs:327,335` — `reduced_cost_base * num / den` and
  `at_date_units * new_units / old_units`. Both are the demerger's two expressions exactly; a
  **write** path, so a panic aborts the exchange.
- `entities::corporate_action::adjustments.rs:343,356` — `qty * new / old` (and its inverse), the
  split re-basing every allocation and report walks through. A parcel of 1e27 units is writable (the
  bound is on `price × quantity`, not quantity), so a four-digit split ratio overflows it.
- `reports::parcel_optimiser.rs:260,262` — `total_proceeds * units_so_far / total_units` and
  `remaining_cost_base * units / remaining_quantity`. The second is `domain::cost_base`'s pro-rate
  re-implemented locally, which is its own finding: it should be calling the shared pipeline.
- `reports::realised_gains.rs:397,450,555` — the sale-cost spread, the scrip-cash apportionment (the
  read-side twin of `scrip_exchange.rs:327`), and the rights-cost spread.
- `reports::activity.rs:647` — `balance * new / old`, the running balance re-based across a split.

The ones that need a figure absurd on its face but not refused anywhere —
`entities::rights_exercise.rs:197`, `entities::corporate_action::adjustments.rs:125,575`,
`reports::tax_summary.rs:425`, `reports::franking.rs:54` and `domain::franking_credit.rs:66` (whose
multiplier is the literal 30, so it needs a franked amount above 2.64e27) — are the same shape and
the same one-line fix, but nothing but scale separates them from the list above.

One related gap the helper cannot close, recorded here so it is not mistaken for part of the sweep:
a demerger or scrip exchange whose ratio is **greater than one** can compute a replacement *quantity*
that is genuinely unrepresentable (1e27 units on a 1000-for-1 ratio). There is no lesser answer to
give, so that is W-e's shape — a write-time refusal naming the arithmetic — not this one.

- [x] Sweep the remaining `a * b / c` sites onto `infra::decimal::mul_div`, with a test at each
      site's ceiling, and decide separately whether the unrepresentable-*quantity* case above wants
      a W-e-style refusal
      — all sixteen substituted, and every one of them was reachable: none turned out to be bounded
      upstream. Fifteen carry an **API-level** test driven through `ApiClient` at the site's own old
      ceiling; one (`contemporaneous_price`) is a unit test, because its only production caller is
      the provider-price re-basing pass, which needs a stubbed fetch and a stored
      `price_as_observed` row before it reaches a single multiplication. The tests:
      `entities::scrip_exchange::tests::{api_exchange_past_the_old_cash_apportionment_ceiling_completes,
      api_exchange_past_the_old_replacement_quantity_ceiling_completes}`,
      `entities::rights_exercise::tests::api_exercise_past_the_old_entitlement_ceiling_completes`,
      `entities::corporate_action::tests::{api_open_parcels_past_the_old_payment_rebasing_ceiling_reports,
      api_open_parcels_past_the_old_split_rebasing_ceiling_reports,
      api_sell_past_the_old_as_acquired_rebasing_ceiling_allocates,
      contemporaneous_price_past_the_old_ceiling_still_recovers_the_day}`,
      `entities::income::tests::api_franking_ceiling_past_the_old_multiply_first_ceiling_is_applied`,
      `reports::activity::tests::api_activity_past_the_old_rebasing_ceiling_carries_the_balance`,
      `reports::franking_at_risk::tests::api_franking_at_risk_past_the_old_apportionment_ceiling_reports`,
      `reports::tax_summary::tests::api_tax_summary_past_the_old_fito_apportionment_ceiling_reports`,
      `reports::parcel_optimiser::tests::{api_optimiser_past_the_old_proceeds_spread_ceiling_reports,
      api_optimiser_past_the_old_cost_base_prorate_ceiling_reports}` and
      `reports::realised_gains::tests::{api_realised_gains_past_the_old_sale_cost_spread_ceiling_reports,
      api_realised_gains_past_the_old_scrip_apportionment_ceiling_reports,
      api_realised_gains_past_the_old_rights_cost_spread_ceiling_reports}`.
      Each was checked by reverting **its own site alone** back to `a * b / c` — not the helper — and
      confirming that test alone dies with `Multiplication overflowed`; every fixture is shaped so
      only the named expression is at the ceiling (nil-priced parcels where the site is a quantity
      re-basing, a $1 sale price where the site is a cost-base pro-rate, and so on), which is what
      makes the per-site revert conclusive rather than merely suggestive.
      Every divisor was traced to a guard rather than assumed: the four corporate-action ratios and
      the scrip cash apportionment's denominator to `CorporateActionBody::kind`'s `positive`
      (`entities/corporate_action/model.rs:492`, applied at 522/523, 539, 554, 583/584, 591/592 —
      and `db.rs`'s `db_upsert` is the only production `INSERT INTO corporate_actions`); the split
      and price-basis ratios to those same figures accumulated as products (`split_ratio`,
      `price_basis_ratio`, whose demerger term is additionally guarded `partly > 0` at
      `entities/closing_price.rs:1175`); `entitled_units` / `parcel_optimiser`'s
      `remaining_quantity` to `domain::open_parcels::load`'s `remaining_as_acquired <= 0` drop
      (`domain/open_parcels.rs:161`); `total_units` to both callers' `units <= 0` refusal
      (`reports/parcel_optimiser.rs:369`, `reports/net_capital_gain.rs:1160`); `sale.quantity`,
      `sale.units`, `entitled_units` and `grossed_up` to the `is_zero`/`> 0` guards standing
      immediately over each expression; and `franking_credit`'s divisor is the literal 70. **No site
      can reach a zero divisor**, so the separate finding that would have been is not raised.
      The multiply-first order stays load-bearing and is still pinned by the tree: swapping `mul_div`
      for a divide-first body fails **thirteen** pre-existing tests (the write-up's twelve, plus one
      the suite has grown since), two of which — `entities::corporate_action::tests::
      db_a_consolidation_that_does_not_divide_still_sells_out_exactly` and
      `reports::realised_gains::tests::db_a_partial_amit_row_leaves_the_units_it_does_not_cover_alone`
      — sit on sites this sweep touched, so the newly swept expressions are order-pinned too and not
      only the three that already were. The whole suite (2,073 tests) is green with no figure moved.
      The unrepresentable-*quantity* decision is **still open** and is now its own TODO section
      rather than being archived closed inside this one.

## A replacement quantity no `Decimal` can hold

Split out of the multiply-before-divide sweep (archived in
[`DONE/tax-domain.md`](DONE/tax-domain.md)), which closed the arithmetic but deliberately left this
decision open: `infra::decimal::mul_div` cannot help here, because the *result* is what is
unrepresentable rather than the working.

A demerger or scrip-for-scrip exchange whose ratio is **greater than one** computes a replacement
quantity of `held × new / old`. On a 1000-for-1 ratio a holding of 1e27 units asks for 1e30
replacement units, which is past `Decimal`'s ~7.9228e28 ceiling however the arithmetic is ordered —
`mul_div` divides early, finds the product still overflows, and panics exactly as the plain
expression did. That is W-e's shape, not the sweep's: there is no lesser answer to give, so the
answer is a write-time refusal naming the arithmetic, in the wording
`domain::cost_base::UnrepresentableCost::message` already uses for a cost base.

**Corrected 2026-08-23 by driving every path against throwaway databases before fixing any of
them. The heading above named three paths — scrip exchange, demerger, transfer — and there are
six, in two shapes; one of the three named is not one of them; and the second shape's refusal
belongs somewhere this section did not say.** The enabling condition throughout is that a parcel of
1e27 units at a **nil** price is a perfectly legal write (`204`): W-e bounds
`average_price × quantity`, which is zero there, so nothing refuses a huge *quantity*.

- **The operation itself panics and nothing is written** — the shape this section described.
  `POST /corporate_actions/:id/exchange` (`at_date_units × new / old`) and
  `POST /corporate_actions/:id/demerge` (`at_date_units × new / held`) each answer a logged `500`
  with an empty body and do not happen. So does the rights issue's **entitlement cap**
  (`held × rights_units / rights_held_units`) — and there the user asked to exercise **100** units:
  it is the cap computation that overflows, not anything they asked for.
- **The write is accepted `204` and then every open-holdings read of the whole portfolio breaks** —
  not mentioned here, and the more serious shape: a stored action bricks the screens until someone
  works out which action did it, with several of the reports that would have found it among the ones
  that are down. `PUT /corporate_actions/:id` with a `ShareSplit` or a `BonusIssue` over such a
  holding, plus a third reproduction of the same hook — a **consolidation** recorded over an existing
  sale allocation, which overflows *inside* the over-consumption re-check that would otherwise have
  refused the write, so it answers a `500` instead of that check's own `422`. This half's refusal
  therefore belongs at the **action write**, beside that re-check, and not in any operation.
- **Transfer is not one of the six, and the section was wrong to name it.** A transfer moves units
  1:1 and applies no ratio of its own: the whole of a 1e27-unit holding transfers `201` with a
  1e27-unit replacement parcel. What it *does* re-base is the units **asked for**, backward into the
  parcel's own basis, which a consolidation multiplies up — so `PUT /transfers/:id` can still reach
  the overflow, but only on a request naming more units than the parcel could ever have held, which
  the over-allocation check would have refused had the re-base not been computed first.

- [x] Refuse a replacement quantity outside `Decimal`'s range at the write, W-e style, naming the
      ratio and the holding that produced it, with a test at the boundary
      — the bound is `domain::cost_base::checked_rebased_quantity`, `mul_div`'s own arithmetic in
      checked form and in the same order (multiply first; where that product alone overflows, divide
      first and multiply after — the headroom `mul_div` exists for), so what is refused is exactly
      what `mul_div` would have panicked on and nothing narrower. Its refusal is
      `UnrepresentableQuantity`, `UnrepresentableCost`'s sibling: both render through one private
      `beyond_the_range(expression, total_is, correct)`, so the sentence and the `Decimal::MAX` quote
      live once and the cost-base wording is unchanged to the byte — only *what the total is* differs
      ("the number of units the holding becomes"). Each path passes its own field labels, so the one
      message reads in the caller's vocabulary: `units held … × scrip_new_units 1000 /
      scrip_old_units 1` on an exchange, `demerger_new_units`/`demerger_held_units` on a demerge,
      `quantity × new units / old units` on a split, `quantity_allocated × old units / new units` on
      the two backward re-bases.
      **Where each refusal sits.** Group A in the operation, before it writes anything:
      `ExchangeError::UnrepresentableReplacementQuantity` and
      `DemergeError::UnrepresentableDemergedQuantity`. Group B at the **action write** —
      `corporate_action::db::WriteError::UnrepresentableRebasedQuantity`, checked over the state the
      write leaves behind by `rebased_quantity_beyond_range`, beside `allocations_fit_parcels` and
      **before** it, since that check computes the very backward re-base that overflows. Both
      directions are covered because both are used: a parcel's gross quantity forward at **every**
      split boundary after it (a split then a consolidation nets back to a ratio that fits while the
      basis in between does not, and a report as at that date still reads it), and each allocation
      backward into its parcel's basis. The transfer's own reach is closed at
      `TransferError::UnrepresentableMovedQuantity`, through
      `corporate_action::checked_as_acquired_quantity` — the higher-level question the module already
      answers, rather than un-gating the raw `split_ratio`.
      **The rights-issue entitlement cap is deliberately *not* a refusal**, and that is the one
      decision here that departs from W-e. That figure is never stored: it exists only to answer
      *have more rights been used than were earned?*, and an unrepresentable cap answers that exactly
      — nothing representable can reach it. There **is** a lesser answer, which is precisely what W-e
      said distinguishes a refusal from a computation, and refusing would deny an ordinary 100-unit
      exercise over arithmetic the user never asked for. So `entitled_units` returns
      `Option<Decimal>`, `None` meaning *unbounded*, and its three call sites (exercise, and the
      rights sale's total and per-parcel caps) read it as such; wherever the cap is representable it
      bites unchanged.
      An **edit** is judged on the terms being *written*, never the stored ones — the check runs
      after the INSERT, on the resulting state — so a database already holding such an action stays
      correctable in place and deletable (`fc1fd7b`'s sibling rule tripped on exactly this, and the
      test fails if the check is moved before the INSERT). Tests:
      `domain::cost_base::tests::{the_rebased_quantity_bound_is_exactly_what_a_decimal_can_hold,
      a_rebased_quantity_is_mul_divs_answer_wherever_that_fits,
      the_quantity_refusal_names_the_ratio_and_the_holding}`;
      `entities::scrip_exchange::tests::api_an_unrepresentable_replacement_quantity_is_refused_naming_the_ratio`;
      `entities::demerger::tests::api_an_unrepresentable_demerged_quantity_is_refused_naming_the_ratio`;
      `entities::corporate_action::tests::{api_a_split_that_rebases_a_parcel_beyond_the_decimal_range_is_refused
      (both the split and its bonus-issue equivalent),
      api_a_consolidation_that_rebases_an_allocation_beyond_the_range_is_refused,
      api_an_already_unrepresentable_action_can_still_be_edited_back_into_range}`;
      `entities::transfer::tests::a_moved_quantity_no_decimal_can_hold_is_refused_naming_it` (its own
      control first: the 1:1 move of the same holding still lands);
      `entities::rights_exercise::tests::a_modest_exercise_against_an_unrepresentable_entitlement_cap_still_lands`
      (which also pins that a representable cap still refuses); plus
      `doc_checks::quantities_beyond_the_decimal_range_documented` over `docs/API.md`'s new
      **Quantities as well as money** subsection, the corporate-action write rule *Writing terms that
      re-base a quantity beyond the decimal range*, the four endpoints' `422` lists, and the
      exception now stated on the **Fractional entitlements** promise. Every one of the eight was
      confirmed to fail with the refusal removed. The pre-existing large-but-representable controls —
      `api_exchange_past_the_old_replacement_quantity_ceiling_completes`,
      `api_demerge_past_the_old_multiply_first_ceiling_completes`,
      `api_exercise_past_the_old_entitlement_ceiling_completes`,
      `api_open_parcels_past_the_old_split_rebasing_ceiling_reports`,
      `api_sell_past_the_old_as_acquired_rebasing_ceiling_allocates` — pass unchanged, which is what
      says the bound is the type's and nothing narrower.
      **Left open, as its own section**: a `ShareSplit`/`BonusIssue` whose ratio is fine when it is
      written can be made unrepresentable later by a parcel entered *behind* it, so the check here is
      necessary but not sufficient — see *A parcel entered behind a ratio that already fits*.

## A parcel entered behind a ratio that already fits

Raised while closing *A replacement quantity no `Decimal` can hold* (archived above), and measured
rather than assumed: the refusal that section added at `PUT /corporate_actions/:id` is **necessary
but not sufficient**.

A `ShareSplit`/`BonusIssue` materialises nothing — its ratio is re-applied at read time — so the
check there can only judge the parcels that exist *when the action is written*. Record a 1000-for-1
split on a listing with no holdings (or small ones): `204`, correctly. Then enter a nil-priced Buy of
1e27 units behind it: `204` as well, because the parcel-creating write bounds
`average_price × quantity` (W-e) and nothing there asks what the listing's recorded ratios would do
to the *quantity*. `GET /portfolio/open-parcels` is then a logged `500` again, and so is every other
open-holdings read of the whole portfolio — exactly the state the action-write refusal exists to
prevent, reached from the other side. Confirmed at the HTTP surface on 2026-08-23.

**Corrected on 2026-08-23 by a second measurement the section did not have.** The headline above is
about the listing the parcel is *entered on*, but a rollover writes its replacement parcels on
another listing entirely, and that listing has ratios of its own. A **1-for-1** scrip-for-scrip
exchange of 1e26 units onto a listing carrying a 1000-for-1 `ShareSplit` answered `201` and then
killed every open-holdings read: `71a26d6`'s operation-level check asks about the *exchange* ratio
and was satisfied, while the destination's ratio was applied at read time afterwards. The demerger
does the same thing with its demerged listing (a 1-for-1 entitlement onto a split-carrying listing:
`201`, then `500`). So the mirror check has to ask about the **destination** listing, not only the
listing the operation is about — the second instance of SCENARIOS V-d's lesson, whose guard had to
cover the destination listing of an exchange or demerger as well as the source.

The mirror check belongs on the parcel-creating writes, asking the same question in the other
direction: *do this listing's recorded re-basing actions leave this quantity representable?* The
machinery is already there — `domain::cost_base::checked_rebased_quantity` and the boundary walk in
`corporate_action::db::rebased_quantity_beyond_range`, which is per-listing and would only need the
about-to-be-written parcel folded into it. The eight parcel-creating paths are the ones
`fc1fd7b` enumerated for the back-dated-parcel rule, so that list is the shape to follow rather than
a fresh grep.

- [x] Refuse a parcel-creating write whose quantity the listing's recorded splits/bonus issues would
      re-base beyond `Decimal`'s range, `422` naming the ratio and the quantity, across every
      parcel-creating path — with the boundary test at the write and a control that the same parcel
      under a representable ratio still lands

**Built** (`docs/API.md`, *Quantities as well as money* → *The mirror: a parcel entered behind a
ratio that already fits*). `corporate_action::db::rebased_quantity_beyond_range` — the very walk the
action write already runs, made `pub` and re-exported — now runs on each parcel-creating write too,
over the state the write leaves behind. Nothing about the walk changed: still both directions, still
at **every** split boundary after the parcel rather than only the cumulative end, so a split then a
consolidation that nets back to a ratio which fits is still refused for the basis in between. The
two hooks together cover the cross product — the action write judges a new ratio against every
recorded quantity, the parcel write judges a new quantity against every recorded ratio. The `422`
body is `UnrepresentableQuantity::message` unchanged, so all nine refusals of the fact word it
identically.

**Seven of the eight paths are reachable, and each was measured at the HTTP surface before anything
was changed.** `PUT /trades/:id` (the headline), `PUT /inheritances/:id` (its own bound is
`cost_base + lpr_expenditure`, a *sum*, which says nothing about a unit count),
`POST /ess_statements/:id/vest` and `POST /corporate_actions/:id/exercise` (their bounds are
`quantity × market_value_per_share` and `exercise_price × units + rights_cost`, which a near-nil
per-unit figure satisfies at any unit count at all), `POST /income/:id/reinvest` **both** ways the
units are arrived at — stated, and derived as `available / reinvestment_price`, the path W-e
deliberately left unbounded because its *product* can never exceed the recorded distribution — and
the replacement parcels of the exchange and the demerge, checked on their **destination** listing.

The eighth, a transfer's transfer-in Buy, is **not** reachable, and is deliberately left unchecked
with the argument recorded beside its error enum. A transfer's destination listing *is* its source
listing, and a transfer-in is dated the transfer date carrying at most the units the source parcel
held then — so every ratio recorded after that date re-bases that parcel by the same factor and at
least as far, while the ratios on or before it apply to the parcel alone. A transfer-in past the
range therefore implies a source parcel past it, which is already refused twice over. Measured as
well as argued: with a 1e26-unit parcel consolidated 1-for-1000 and its whole holding transferred,
the later split that would take the transfer-in past the range is refused for taking the *parcel* it
came from there — quoting the parcel's 1e26, which is what says the source is what bounds it
(`db_a_transfer_in_is_bounded_by_the_parcel_it_moves_from`).

The demerger's **head** replacements are excluded by the same argument and for the same reason, so
only the demerged listing is walked.

**Editability.** The walk runs *after* each write's own INSERT, over the resulting state, exactly as
`71a26d6` and `fc1fd7b` do — so a parcel already stored beyond the range (only a build predating
this rule could have written one) is corrected by the very write that would be refused if the walk
ran first, and is still deletable. Verified the way `71a26d6` verified its own:
`api_an_already_unrepresentable_parcel_can_still_be_corrected_and_deleted` fails when the check is
moved ahead of the INSERT. Every refusal test was also confirmed to fail with the refusal
neutralised, and every control to pass either way.

Tests, each driven through `test_support::ApiClient`: `entities::trade`
(`api_a_parcel_behind_a_ratio_that_already_fits_is_refused_naming_it`,
`api_a_parcel_unrepresentable_only_between_two_ratios_is_refused`,
`api_a_large_parcel_a_recorded_ratio_still_fits_lands_and_reports`,
`api_an_already_unrepresentable_parcel_can_still_be_corrected_and_deleted`), `entities::inheritance`
(`api_an_inherited_quantity_a_recorded_ratio_rebases_out_of_range_is_refused` + its control),
`entities::ess_vest` (`vest_of_a_quantity_a_recorded_ratio_rebases_out_of_range_is_refused` + its
control), `entities::rights_exercise`
(`api_an_exercised_quantity_a_recorded_ratio_rebases_out_of_range_is_refused` + its control),
`entities::drp_reinvestment`
(`api_a_reinvested_quantity_a_recorded_ratio_rebases_out_of_range_is_refused`,
`api_a_derived_reinvested_quantity_beyond_the_range_is_refused_too` + its control),
`entities::scrip_exchange`
(`api_a_replacement_parcel_the_destination_listings_own_ratio_rebases_out_of_range` + its control),
`entities::demerger` (`api_a_demerged_parcel_the_demerged_listings_own_ratio_rebases_out_of_range` +
its control), `entities::transfer` (`db_a_transfer_in_is_bounded_by_the_parcel_it_moves_from`), and
`doc_checks::a_parcel_entered_behind_a_ratio_that_already_fits_documented`. Every control's figures
were captured from the pre-change build and pinned unchanged: 7.9e25 units behind the same real
1000-for-1 ratio re-base to 7.9e28, inside `Decimal`'s ~7.9228e28 ceiling, and every read reports
them exactly as before.


## SCENARIOS Z-g — an AMIT taken over mid-year has its final cost-base reduction recordable nowhere

- [x] Accept an AMIT adjustment against a cross-listing rollover replacement parcel, as the refusals already promise.

Found while fixing [Z-d](DONE/reporting.md), and confirmed independently against a throwaway database.
`docs/API.md`'s [AMIT adjustments](docs/API.md#amit-adjustments) section states the rule plainly:

> a **rollover replacement parcel** whose units trace back to the statement's own account through a
> [transfer], [scrip-for-scrip exchange] or [demerger] *is* accepted, wherever the operation moved
> them … the chain is followed

It is not followed across a change of listing. `entities::amit_adjustment`'s write-time check refuses
any row whose trade's `listing_id` differs from the statement's, before the account-tracing reach-through
is consulted — so the promise holds for exactly the replacements that happen to stay on the same listing:

| replacement parcel | same listing? | row accepted? |
| --- | --- | --- |
| a [transfer](docs/API.md#transfers)'s transfer-in parcel | yes | `204` ✔ |
| a [demerger](docs/API.md#demerging)'s **head** replacement | yes | `204` ✔ |
| a [scrip-for-scrip](docs/API.md#exchanging-a-scrip-for-scrip-takeover) replacement | **no** | `422` *the trade's listing differs from the AMMA statement's listing* |
| a demerger's **demerged-entity** parcel | **no** | `422` *(same)* |

**The two refusals point at each other, so there is no way through.** An AMIT fund taken over part-way
through a financial year, whose final AMMA statement arrives months later stating nil units held:

1. `POST /amma_statements/:id/generate_adjustments` → `422` — *"…if it was transferred, exchanged or
   demerged away during the year, enter them against the replacement parcels that now hold those units,
   **which is accepted** because the units trace back to this account"*
2. the row against the **replacement** parcel → `422` — *"the trade's listing differs from the AMMA
   statement's listing"*
3. the row against the **original** parcel → `422` — *"…Enter the rest against the replacement parcel
   instead, where those units now are: **that is accepted** for a statement of the account they came
   from, and generating the statement's set does it for you"*

Each refusal names the other as the way out. The statement's `cost_base_adjustment` — the fund's whole
final-year AMIT cost base net amount, CGT event E10 — can be recorded **nowhere**, so the replacement
parcel's cost base stays overstated by it and every later disposal of those units understates the gain.
In the reproduction that is 10,000 units × A$0.20 = **A$2,000** of cost base that should have come off.
`generate_adjustments`'s `unattributed` list exists precisely to hand these rows to the user for manual
entry, and manual entry is what is refused.

**It fails loudly rather than silently**, which is the good half — nothing computes a wrong figure — and
the [AMIT adjustment cross-check](docs/API.md#amit-adjustment-cross-check) then flags the statement as
having no adjustments, forever. But the documented recovery does not exist.

**Direction.** The account-tracing reach-through is already written and already used; the listing check
simply runs first and unconditionally. Consult it before refusing: a parcel the rollover chain shows
holds this statement's account's units is acceptable *whatever* listing it now sits on — that is the
whole point of a scrip-for-scrip exchange. Keep refusing a parcel that never held the account's units,
which is what the check is really for.

**Fixed.** The listing check no longer runs first and unconditionally: both pins — the statement's
listing *and* its holding account — are now reached through by one rule in
`entities::amit_adjustment`'s `db_write_on`, which accepts a parcel when `domain::rollover`'s
existing chain walk (`source_ancestors`, the same one the per-account reach-through already used)
shows a source parcel of the statement's own (listing, account). No second walk was written; the
one that was there is asked for both halves of the identity instead of the account alone, so a
holding exchanged and then transferred is reachable through both hops. A parcel with no such path
is refused exactly as before, and both refusals now name what would have made it acceptable.

Two things had to move with it. Generation's `db_rollovers_of` filtered a rollover group's
replacement parcels to the **source listing**, so the `unattributed` list — the very list that
hands these rows to the user — named nothing at all for a scrip-for-scrip exchange; it now carries
every replacement of the group with its listing, while `Movement::matching_replacement` still only
ever identifies a transfer's own same-listing replacement. And the cross-check's coverage band
measured its disposal allowance on the adjusted parcels alone, which on a replacement finds no
disposal: the units left the statement's account through the *operation's* closing Sell, recorded
against the source parcel. It now follows the same `source_ancestors` chain, so the accepted entry
is not flagged for the rest of time (this also silences a pre-existing false flag on the
transfer-during-the-year case, whose test said so in a comment).

**The quantity cap is deliberately unchanged: the parcel's own units.** `quantity` keeps one
meaning everywhere — units of the parcel the row is against, in its as-acquired basis — because
the whole cost-base pipeline is built on `covered <= parcel quantity`:
`AmitReductionEvent::reduction_for_units` splits a row's total between the units still held at the
statement's year end and the units sold before it, and coverage beyond the parcel would spill onto
units that do not exist, silently delivering less than the row states wherever nothing was disposed
of by the year end — which is precisely the mid-year-takeover case. The consequence, documented in
`docs/API.md` and commented at the check: the reduction a row applies is always `quantity` × the
statement's per-unit figure, so on a replacement whose unit count an exchange or demerger ratio has
scaled, the statement's figure has to be re-expressed per replacement unit — the same entry the
docs already prescribe for a statement stating a total rather than a per-unit amount. The ATO
states the AMIT cost base net amount as one annual, member-level amount and prescribes no spread
across parcels (`docs/ato/amit-cost-base-adjustments.md`), so apportioning it across a demerger's
two legs is the member's own working and nothing here infers it.

Tests — accepted: `entities::amit_adjustment::db_an_amit_taken_over_mid_year_adjusts_its_cross_listing_replacement`
(the finding's own reproduction: statement stating nil units, row against the cross-listing
replacement, A$2,000 reaching that parcel's cost base in the open-parcels figures),
`api_a_cross_listing_replacement_is_accepted_and_an_unrelated_parcel_is_not`,
`db_a_demerged_entity_parcel_is_adjustable`,
`db_a_two_hop_chain_across_a_listing_and_an_account_is_followed`,
`entities::amit_adjustment_generation::api_generation_names_a_cross_listing_replacement_and_that_row_is_accepted`
(generation names the replacement and the named row is accepted, end to end), and
`reports::amit_adjustment_cross_check::db_a_mid_year_takeovers_replacement_row_is_not_flagged`.
Still refused: `db_a_parcel_that_never_held_the_statements_units_is_still_refused` (a parcel of the
acquirer's listing bought on market, in the statement's own account), the negative control inside
the API test above, and the pre-existing `db_listing_mismatch_rejected` /
`db_holding_account_mismatch_rejected` / `db_a_replacement_parcel_in_another_account_is_adjustable`
unchanged.

## SCENARIOS AA-a — an indexation-eligible parcel is silently costed on the discount, and the reason given for not modelling it is false for a wide, enterable range

- [x] Decide and implement (options below).

Scenario AA-02. `docs/API.md`'s [Known limitations](docs/API.md#known-limitations) justifies the
scope cut this way:

> **Indexation method** (2026-06-10) — for an asset acquired before **21 September 1999** an
> individual may index the cost base for inflation (frozen at the 30 September 1999 CPI) *instead of*
> applying the 50% discount … The discount **almost always** gives an individual the better result,
> so indexation is not modelled — the 50% discount is used throughout.

**"Almost always" is not true of the parcels this system can actually hold.** The earliest
enterable acquisition is **1985-09-20** (AA-01's own floor), and the indexation factor for a
September 1985 quarter cost is 68.7 ÷ 39.7 ≈ **1.731** — so indexation wins whenever the proceeds
are below about **2.46 × cost**, which over a forty-year hold is an ordinary outcome, not an edge
case. (The ATO's own page says only "in most cases", and adds the loss caveat that widens the range
further: `docs/ato/indexing-the-cost-base.md`, "Indexation may give you a better result in some
situations, such as if you also have capital losses.")

Driven: a parcel bought 1985-09-20 for A$10,000, sold 2025-06-02 for A$20,000.

| method | assessable gain |
| --- | ---: |
| 50% discount (what the system reports) | **A$5,000.00** |
| indexation (10,000 × 1.731 = 17,310 indexed cost) | **A$2,690** |

`GET /portfolio/net-capital-gain` reported the year's `cgt_discount` of `5450.00` and
`net_capital_gain` of `5450.00` with this parcel's A$5,000 inside it. **Nothing anywhere names the
alternative** — no report field, no cross-check row, no health entry mentions indexation for a
directly held parcel. (The word is not absent from the tree: an AMMA statement already carries
`cgt_indexation_gains`, so the *trust* side of the indexation method is modelled and reported while
the taxpayer's own election is invisible.)

**This does not compute a wrong number** — the discount method is a lawful choice, so the reported
figure is defensible — but the taxpayer is never told a cheaper lawful choice existed, and the
documented reason for withholding it is wrong for exactly the parcels most likely to be affected.

> **Note for whoever fixes this:** re-derive the September 1985 CPI (39.7) and the factor from the
> ATO's own published table rather than from this write-up — per the standing lesson, a finding's
> arithmetic is not evidence. The rounding rule is "limited to 3 decimal places, round the fourth
> decimal up from 5".

**Options.**

1. **Flag, don't compute.** Mark every disposal parcel acquired before 21 September 1999 as
   indexation-eligible on the realised-gains / net-capital-gain / annual-tax-report parcel rows, and
   add a cross-check (or health) row naming each affected disposal with the two figures side by side
   so the taxpayer can see which method wins. Correct the Known-limitations wording to state the
   actual boundary rather than "almost always". Cheapest honest fix; the arithmetic stays the
   taxpayer's own adjustment, exactly as K10/K11 does — and K10/K11's `settlement_crosses_rate_month`
   is the existing precedent for reporting an omission on the data.
2. **Model it.** A frozen ATO quarterly CPI table (seeded, ~56 rows to September 1999), an indexed
   cost base through `domain::cost_base`, and a per-parcel election reported both ways with the
   better taken. Substantial: indexation is forbidden on a capital loss and cannot be combined with
   the discount, so the net-capital-gain loss-netting walk has to choose per parcel, and the choice
   interacts with the brought-forward loss chain.
3. **Documentation only.** Correct the "almost always" claim and state the crossover, add nothing to
   any report.

**Chosen: option 1 — flag, don't compute.** Clarified 2026-08-25: *both figures side by side*. The
frozen ATO quarterly CPI table is seeded and an indexed cost base computed, so the advisory row can
show the two methods against each other — but **no reported tax figure changes**: the net capital
gain, the annual tax report and every CSV export stay on the 50% discount throughout. The indexed
figure exists only to answer "which method wins here", which is the question the finding is about.

**Fixed.** The frozen ATO quarterly CPI series is seeded as `cpi_quarters` (migration
`0046_cpi_quarters.sql`, 57 rows — the September 1985 quarter through the September 1999 freeze and
deliberately nothing after it), mirrored from Appendix 2 of the *Guide to capital gains tax 2025*
(QC 104764) in [`docs/ato/consumer-price-index.md`](docs/ato/consumer-price-index.md) and indexed in
`docs/ato/OVERVIEW.md`. `domain::indexation` holds the method's arithmetic — the eligibility
boundary, the quarter mapping, the factor (68.7 ÷ the quarter's CPI, limited to 3 decimal places
with the fourth decimal rounded up from 5), and the indexed cost base — and
`domain::cost_base::CostBase` gained `costed_initial_cost`, the costed units' share of the initial
cost base, which is the only *indexable* component and cannot be reconstructed from the netted
total once CGT event E10/G1 has floored it at nil. `reports::realised_gains` computes the indexed
figure per allocation and carries it on the parcel rows as `indexation_eligible` +
`indexed_cost_base`, the annual tax report notes it under an eligible parcel on the archived
document, and `GET /reports/indexation_cross_check` sets both methods' assessable gains against
each other per parcel and per year. **No reported tax figure moved**: the same disposal driven
through both builds — pre-change and post-change, same facts, same throwaway database — answers
byte-identical net capital gain, realised gains, tax summary and annual tax report once the two new
advisory fields are stripped.

**The finding's arithmetic was wrong in one place, and it is the place it warned about.** The
September 1985 factor is **1.730**, not 1.731: 68.7 ÷ 39.7 = 1.730478…, whose fourth decimal is a 4,
so the ATO's "round the fourth decimal up from 5" rounds it *down*. 1.731 is what the **superseded
1989-90-base** series gives (123.4 ÷ 71.3 = 1.73070…), and the ATO marks that table "no longer
[usable] for tax and super purposes". So the finding's A$2,690 indexed gain is really **A$2,700**
(A$10,000 → A$17,300 indexed). The **2.46× crossover was right** — with a 1.730 factor indexation
wins below exactly 2.460 × cost, driven at A$24.59 / A$24.60 / A$24.61 per unit against a A$10 cost
and answering Indexation / Equal / Discount. Both of the ATO's own worked-example factors (Val:
1.164 and 1.159) reproduce against the seeded table, which is what says the table *and* the rounding
rule are right rather than merely self-consistent.

**How the comparison is stated, and why.** Per **parcel allocation**, and explicitly *before any
capital losses applied against the gain* — stated in the module doc, in `docs/API.md`, and on every
year's own `comparison` row so the qualifier travels with the figures into a printout. Per parcel
because that is the only level at which it is a fact rather than an assumption: a parcel is a
separate CGT asset, and one Sell can draw on a 1998 parcel and a 2015 one whose methods differ.
Before losses because the two methods do not meet losses at the same point — losses come off the
gross gain and the discount applies to what is left, while an indexed gain has no discount to follow
— so writing `g` for the gross gain, `r` for the indexation uplift and `L` for the losses applied,
indexation's advantage is `r − (g − L) / 2`, which **rises** with `L` until `L` reaches `g − r` and
both methods reach nil together. Applying losses therefore never moves the answer toward the
discount, which makes every row a **floor** on indexation's case rather than the whole answer; each
year row carries the capital losses the year actually realised so a reader can see whether the
qualifier bites. Two exclusions decided rather than assumed: a parcel disposed of at a **loss** is
left out entirely rather than shown as "discount wins" (indexation cannot be used on a capital loss
at all — its loss still reaches its year's `capital_losses_realised`), and a **rights sale** is left
out because what would be indexed is the rights' own cost base, nil for the free rights modelled
here. Eligibility is tested on the parcel's own **trade date** — when the cost was incurred — never
on the deemed acquisition date the discount clock runs from, since an inherited or
rollover-replacement parcel has its own indexation rules and none of them are modelled. Costs
incurred after the cut-off cannot arise on the indexable side: the AMIT (E10) and return-of-capital
(G1) movements are *reductions*, not costs, so they come off the indexed figure at face value (the
conservative direction), and a disposal's own brokerage is netted from proceeds rather than added to
the cost base.

`cpi_quarters` is classified **exempt** for snapshot staleness (nothing writes it after its
migration, and no snapshotted report reads it) and deliberately **not audited** (`row_history` exists
to recover a user's edit of a financial fact; this table has no write path at all — contrast
`exchange_holidays`, audited precisely because it has a DELETE route). The *Indexation method*
Known limitation is rewritten: the "almost always" claim is explicitly withdrawn and replaced by the
1.730 factor and the 2.460 × cost crossover, with the scope cut narrowed to the **election** —
choosing per parcel, and that choice's interaction with the loss-netting walk — which stays out of
scope. `docs/API.md` gains the report's own section, `docs/SCHEMA.md` the table and its
relationships entry, `README.md` a feature line and a corrected scope clause, and `src/doc_checks.rs`
two pins (one of which asserts "almost always" survives in exactly one place: the sentence
withdrawing it).

## SCENARIOS AA-b — a non-renounceable rights issue is indistinguishable from a renounceable one, and its retail premium is recorded as a capital gain

- [x] Decide and implement (options below).

Scenario AA-13. The two treatments of a retail premium turn entirely on whether the offer was
renounceable, and `docs/ato/retail-premiums.md` states the split plainly: under a **renounceable**
offer the premium is capital proceeds on the rights (TR 2017/4), and under a **non-renounceable**
offer it is an **unfranked dividend** (TR 2012/1) — "enter it as unfranked dividend `income` against
the listing, not as a corporate action or rights sale."

**The `RightsIssue` corporate action records no such fact.** Its fields are `rights_units`,
`rights_held_units`, `exercise_price` and `currency` — there is no `renounceable` column anywhere in
the tree (`grep -ri renounce src` finds only prose). A non-renounceable entitlement offer is a
perfectly legitimate thing to record, because *exercising* one is identical either way and the
exercise path is the reason to enter the action at all. Having entered it, `sell_rights` is offered,
and it accepts:

```
PUT  /corporate_actions/1  {"action_type":"RightsIssue", ... }          → 204
POST /corporate_actions/1/sell_rights
     {"units":"250","proceeds_per_right":"0.20", ... }                  → 201
```

A$50 of retail premium is now a **capital gain** — halved again if the anchoring parcel is past
twelve months, since free rights inherit the original shares' acquisition date — where TR 2012/1
makes it fully assessable unfranked dividend income at item 11S. Wrong amount and wrong return label,
with nothing asked and nothing said. The endpoint's own documentation says "under this
(**renounceable**) offer" and the UI's action description says "under this renounceable offer" — both
*assume* the fact neither collects.

**Options.**

1. **Record it and refuse the wrong path.** Add `renounceable: bool` to the `RightsIssue` action
   kind (defaulting existing rows to renounceable, which is what every stored row means today), and
   have `sell_rights` refuse `422` on a non-renounceable offer when `proceeds_per_right` is positive
   — naming TR 2012/1 and pointing at the income path. A **nil**-proceeds lapse stays accepted: a
   non-renounceable right can lapse, and at nil/nil it is a non-event either way.
2. **Record it and flag it.** Add the column and surface a cross-check row for every rights sale
   with positive proceeds against a non-renounceable offer, refusing nothing.
3. **Documentation only.** Add a Known-limitations line saying the action assumes a renounceable
   offer, and that a non-renounceable premium is entered as unfranked income instead.

**Chosen: option 1 — record `renounceable` and refuse the wrong path.**

**Fixed.** `ActionKind::RightsIssue` carries `renounceable` (migration 0047: an INTEGER 0/1 column,
CHECK-confined to `RightsIssue` rows, every stored row backfilled to renounceable — what they all
already meant — with `corporate_actions`' two row-history triggers dropped and re-created around it).
It is **required** on the PUT body and forbidden on every other action type, not defaulted: the
whole finding is that the fact was never asked for, and a quiet default would have left the same
assumption in place for every new entry. (The complementary CHECK — a rights issue *must* carry the
flag — is the one part SQLite cannot express by `ALTER TABLE ADD COLUMN`, since it evaluates a new
CHECK against the rows already there and would reject the very rows 0047 backfills; it lives in
`CorporateActionBody::kind`, beside this table's other write-time rules a CHECK cannot express, and
the migration header says so.)

`sell_rights` now refuses **two** things against a non-renounceable offer, both `422`: a positive
`proceeds_per_right` (naming TR 2012/1 and pointing at unfranked dividend income) and a positive
`rights_cost`. The second was the open question in the write-up, and the ruling answers it: TR
2012/1's scheme is defined by entitlements that "**cannot be traded, transferred, assigned or
otherwise dealt with**" (para 2), so nothing can have been *bought* either — an unchecked cost would
realise a capital loss on the lapse out of an amount that was never paid. A **nil/nil lapse stays
accepted** and still consumes the entitlement. The premise held up too: exercising is identical
under both offers (`docs/ato/rights-issues.md`'s rules turn on how the rights were acquired and on
the original shares' pre/post-CGT status, never on renounceability), so recording a non-renounceable
issue in order to exercise it is the normal case, and `rights_exercise` was deliberately left alone
with a comment saying why.

Re-derived rather than taken from the write-up, and it moved two things: QC 21832 had been
restructured by a 22 June 2026 update, so `docs/ato/retail-premiums.md` was re-fetched in full
(2026-08-25) and the drift recorded in `docs/ato/OVERVIEW.md`; and TR 2012/1 itself was fetched and
quoted into that mirror, which is where the non-tradeable definition and paras 9–11 come from — the
premium is **not** partly capital, since CGT event C2 does happen on the right to it but s 118-20
reduces the gain by whatever is assessed as income. One documented caveat the finding did not have:
TR 2012/1 expressly does not consider entitlement offers over **trust or stapled-group** equity, so
for those the refusal still holds (the entitlements are non-tradeable either way) but the payment's
character is whatever the distribution statement says.

`docs/SCHEMA.md`, `docs/API.md` (the `RightsIssue` description, *Selling or lapsing rights* with its
new `422`, and the Known-limitations bullet, which no longer says non-renounceable premiums "are not
modelled" — the wrong path is refused and the right one named) and `README.md` were updated with it,
and `src/web/config.js` asks for the flag (a checkbox defaulting to renounceable) and stops
asserting "under this renounceable offer" — the sell-rights screen of a non-renounceable issue now
opens by saying only a lapse can be recorded there. Tests: the two refusals and the accepted lapse
(DB and API level, the API one pinning the wording), a renounceable offer unaffected, both
round-trips through `PUT`, the flag required/forbidden, and `migration_0047_…` proving an existing
row reads back as renounceable through the model.

### AA-b, second item — the exercise path still accepts a cost on a non-renounceable offer

- [x] Refuse `rights_cost` on `POST /corporate_actions/:id/exercise` for a non-renounceable offer.

Flagged by the agent that fixed AA-b and deliberately left unenforced there; **decided 2026-08-25 to
refuse it now.** `sell_rights` refuses a positive `rights_cost` on a non-renounceable offer because
TR 2012/1 para 2 defines the scheme by entitlements that "cannot be traded, transferred, assigned or
otherwise dealt with" — so nothing can have been paid to acquire them. The same fact makes the amount
impossible on the **exercise** path, which still accepts it. The consequence is smaller and more
visible than `sell_rights`' was (a stray cost inflates the new parcel's cost base rather than
fabricating a capital loss out of money never paid), but it is the same impossible amount, and the
guard should not hold on one path and not the other.

**Fixed.** `db_exercise` now reads the offer's `renounceable` flag and returns `422` on a positive
`rights_cost` against a non-renounceable one, before anything is written. The premise was re-derived
first and held: `docs/ato/rights-issues.md`'s exercise rules give `rights_cost` the *same* meaning on
both paths — the cost base of the rights at exercise, "including any amount you paid for them" —
and `docs/API.md` says it covers rights **bought on-market**, which TR 2012/1 para 2's entitlements
(not tradeable, transferable or assignable) cannot be. The para 3 caveat does not open a legitimate
case: the Ruling declines to characterise the *payment* on trust/stapled-group offers, but a
non-renounceable entitlement is non-tradeable there too, so the cost stays impossible; and nothing
legitimate is blocked, since the exercise itself is unaffected — at the nil cost a free entitlement
carries it lands exactly as before.

The predicate is shared as far as it should be: the *fact* both refusals rest on now lives once, as
`corporate_action::NOTHING_PAID_FOR_NON_RENOUNCEABLE_RIGHTS`, and each `From<EntityError> for
ApiError` arm follows that clause with what the amount would have done to *its* figures (a capital
loss on a lapse for `sell_rights`; an inflated parcel cost base here) — so the two read as one rule
in two places without either call site being contorted into the other's shape. The check itself is
two tokens against a flag each path already has in hand and was not worth a helper.

`docs/API.md` (the exercise section's new `422` and its `rights_cost` description, the `RightsIssue`
description, the *Selling or lapsing rights* cross-reference that used to say the exercise path was
deliberately untouched, the Known-limitations rights bullet, and the response-codes catalogue),
`README.md`'s rights-issue line, and `src/web/config.js` (the exercise screen's description and
rights-cost hint, mirroring the sell-rights screen) carry it. No migration: the fact was already
recorded by 0047. Tests: the refusal and a nil-cost exercise of the same offer (DB and API level, the
API one pinning the wording and that only the nil-cost exercise was written), plus a renounceable
offer with a positive cost still costing the parcel `500.05`.

## SCENARIOS AA-c — the investor-not-share-trader assumption is stated nowhere

- [x] Decide and implement (options below).

Scenario AA-07. Every figure this system produces assumes the holdings are **CGT assets held on
capital account**. For a **share trader** carrying on a business, shares are **trading stock**: gains
and losses are ordinary income and deductions, there is no CGT event, no 12-month discount, no
capital-loss pool and no 18V carry-forward, and closing stock is valued at year end instead.

`docs/API.md`'s Known limitations has **32 bullets and none of them says this.** The closest,
*Taxpayer entity type*, is about a different axis entirely — individual vs SMSF vs company vs trust,
and the *rate* of the discount — and a share trader is very often exactly the individual resident
that bullet describes. `grep -in "trading stock\|share trader" docs/API.md README.md` returns
nothing; the phrase appears only inside a mirrored ATO page (`docs/ato/worthless-shares.md`'s G3
eligibility list).

This is the one boundary in section AA with **no documented limitation behind it at all**, and the
consequence is not a rounding: a trader who used this tool would lodge capital gains — half of them
discounted away — where ordinary income belongs, and carry forward capital losses that should have
been deductions.

Nothing can detect it, so a refusal or a flag is not available; the fix is that the assumption is
written down.

**Options.**

1. **Its own Known-limitations bullet** ("Investor, not share trader"), a README scope line beside
   the other named scope cuts, and a `doc_checks` test pinning both.
2. **Fold it into the existing *Taxpayer entity type* bullet** as a second paragraph, pinned the
   same way.

**Chosen: option 1 — its own Known-limitations bullet, README line, and `doc_checks` test.**

**Fixed.** `docs/API.md`'s Known limitations gained its own **Investor, not share trader**
(2026-08-24) bullet, placed directly after *Taxpayer entity type* with the two cross-referenced so
neither reads as covering the other (that one is *which taxpayer* and the rate; this one is whether
the CGT machinery applies at all). It states the trading-stock treatment concretely — profit on sale
assessable as ordinary income, purchase price and transaction costs deductible in the year incurred
so there is no per-parcel gain at all, losses deductible against income from any source, and
Division 70's year-end stock adjustment (s 70-35 / s 70-45) not modelled anywhere — what that costs
a trader who ran these reports (half each year's profit exempted as a discount at 18A, losses parked
at 18V instead of claimed, brokerage capitalised that was already deductible), and that **nothing can
detect it**: the test is how the activities are carried on, not anything on a trade row, so there is
no refusal, no health flag and no `taxpayer_basis`-style marker — the assumption is written down
instead of enforced. Two things the drafting of this finding did not have: the income side is
**unaffected** (dividends and similar receipts are assessable either way, so the tax summary's
dividend, franking and expense lines hold for a trader too), and 18V is not simply unavailable to a
trader — an investor→trader change keeps unused prior-year capital losses as **capital** losses,
which can never become revenue losses. The change-of-status rules are named as out of scope too
(**CGT event K4** where the change elects market value; the trader→investor deemed sale at cost),
with the manual Sell + Buy that is all an entry path could be.

The ATO source was re-derived rather than taken from the finding: QC 66047 *Share investing versus
share trading*, mirrored as `docs/ato/share-investing-versus-share-trading.md` (retrieved
2026-08-24) with its investor/trader table, carrying-on-a-business factors, the George example and
both change-of-status paths, and indexed at the head of `docs/ato/OVERVIEW.md`'s CGT table as the
threshold question the rest of it sits on. `README.md`'s scope-cut paragraph carries the matching
line, and `doc_checks::known_limitations_document_the_investor_not_share_trader_assumption` pins
both halves plus the mirror's QC number and its two load-bearing sentences.

## SCENARIOS AA-e — four limitations are documented without the workaround that exists and works

- [x] Decide and implement (options below).

Scenarios AA-06, AA-08, AA-12, AA-19. Each of these bullets states what is *not* modelled and stops
there, while a correct entry convention exists — and in three of the four it was driven and works.
The pattern the file already uses elsewhere is the opposite: the *DRP partial participation*,
*Gifts*, *Rollovers assume the rollover was chosen* and *multi-year expense* bullets each name their
workaround, and the *Inherited parcels* bullet even prescribes the "enter your own share" convention
that AA-06 needs.

- **AA-06, joint ownership** — *One taxpayer* says the ownership dimension is not modelled and gives
  no remedy. Driven: a 50% joint interest entered as **your own half** — 500 units at $10 — costs
  A$5,000 and reports correctly throughout, with `amount_per_security` / `securities_held` keyed to
  your half rather than the registry statement's whole. This is the same convention *Inherited
  parcels* already prescribes for a parcel split between beneficiaries.
- **AA-08, cost-base elements** — the bullet reads "elements 1 (acquisition) and 2 (incidental costs:
  brokerage + GST) **are captured**", which over-states element 2: the ATO's element 2 also covers
  stamp duty, transfer costs, and remuneration for professional advice on the acquisition, none of
  which has a field. Driven: A$500 of off-market transfer stamp duty entered as `brokerage` with the
  reason in `contract_note_ref` lands in the cost base exactly (100 units at $10 plus $500 →
  A$1,500). It is the right answer arithmetically and is documented nowhere.
- **AA-12, Div 775 forex on a foreign-currency cash balance** — documented only as a **clause inside
  the *Crypto assets* bullet** ("Foreign-currency cash balances (Div 775 forex gains — ordinary
  income, not CGT) are deferred to a separate specification"), where a reader looking for
  foreign-currency scope will not find it. And unlike the others there is **no** workaround: an
  [income](docs/API.md#income) row requires a `listing_id`, and a cash balance has no listing, so a
  Div 775 gain has nowhere to be entered at all. The doc does not say so.
- **AA-19, a second taxpayer** — *One taxpayer* again, with no remedy stated. The remedy the tool
  already supports is **one database and one instance per taxpayer** (`--db`, `--port`), which is
  worth naming precisely because the wrong answer is so easy: a spouse's holdings entered as a second
  holding account aggregate into one net capital gain, one loss pool, one A$5,000 franking threshold
  and one A$1,000 FITO de-minimis, silently wrong for both people.

**Options.**

1. **Add the workaround to all four bullets**, each pinned by a `doc_checks` test — including
   AA-12's honest "there is no entry path", moved out of the Crypto bullet into one of its own.
2. **A subset** — say which.

**Chosen: option 1 — all four, with AA-12 promoted out of the Crypto bullet into one of its own.**

**Fixed.** All four bullets in `docs/API.md`'s Known limitations now carry their convention, each
pinned by its own `doc_checks` test, and every claim was re-driven against a throwaway database
before it was written down.

*One taxpayer* was rewritten once and carries both remedies (AA-06 and AA-19 live on the same
bullet, and splitting them would have left two halves each needing the other's context). **A second
taxpayer is a second database and a second instance** — one server per taxpayer, `--db` and `--port`
apart — with the wrong answer named: a spouse entered as another holding account aggregates into one
net capital gain, one loss pool, one A$5,000 small-shareholder franking threshold and one A$1,000
FITO de-minimis, wrong for both people and **not in one predictable direction** (pooled losses
understate one person's gains, while a combined franking or foreign-tax total tips both out of
thresholds neither reached alone — the finding's drafting had only the understating half). Nothing
can detect it, because aggregating what is in the database is what the reports are for. **A jointly
held parcel is entered as your own share** — 50% of a 1,000-unit holding is a 500-unit Buy, costing
A$5,000, verified through `/portfolio/open-parcels`. One correction to the finding's write-up: it is
**not** both per-share figures that are keyed to your half. `amount_per_security` stays the
statement's per-unit rate and only `securities_held` is your own unit count — the cross-check is
`amount_per_security × securities_held` against the entered cash, so $0.20 × 1000 against your half's
$100 is the `422` and $0.20 × 500 is accepted. Cross-referenced to *Inherited parcels*, with the cost
stated: your unit counts deliberately will not tie back to the registry's holding statement.

*Cost base elements* no longer claims element 2 is captured: element 1 has a field and element 2 has
**one** field. The other element-2 costs were re-derived from `docs/ato/cgt-cost-base.md` rather than
from the finding — costs of transfer, stamp duty or other similar duty, remuneration for a broker,
agent, accountant, consultant or legal adviser (tax advice only from a recognised tax adviser,
incurred after 30 June 1989), a valuation or apportionment made to work out the gain, and expenses
incurred as a direct result of ownership ending — with the ones a *listed-share* investor actually
meets named. The convention (fold it into `brokerage`, say what it was in `contract_note_ref`) is
exact: 100 units at $10 plus A$500 of transfer duty reports a A$1,500 cost base, and nothing bounds
the fee above. Driving it turned up **two traps the finding did not have**, both now documented and
pinned: `brokerage_includes_gst` would ÷11-split a A$500 duty into A$45.45 of GST that never existed
(the cost base is identical, the GST column is not), and a supplied `statement_total` is reconciled
against `quantity × price ± (brokerage + GST)` at write time, so a broker-note total that omits
separately paid duty is a `422` naming the computed figure. The disposal-side asymmetry points at
*Where a Sell's brokerage and GST land*, and the one thing with no home at all — an element-2 cost
belonging to no single trade — is stated.

*Div 775 forex on a foreign-currency cash balance* is now its own bullet, sited immediately after
*Settlement-window forex — CGT events K10/K11*, and says plainly that **there is no entry path at
all**: an [income](docs/API.md#income) row's `listing_id` is a required `i64` (verified —
`IncomeBody`, and both a missing and a `null` one answer `422`) and a currency balance has no
listing, so the gain has nowhere to go; the loss side is no better, since an investment expense would
report a forex loss on a line it does not belong to. Cited to `docs/ato/forex-common-transactions.md`
(QC 18322, s 775-15). The *Crypto assets* bullet keeps its load-bearing half — the deferral never
reaches a crypto holding, TD 2014/25 and the 2023 statutory exclusion — and the two bullets now
cross-reference each other instead of one carrying the other's scope.

`README.md`'s scope-cut paragraph gained two clauses (one taxpayer per database, and the
foreign-currency cash balance with no entry path); the element-2 convention stayed out of it as
entry-convention detail rather than a scope cut. Four new tests in `src/doc_checks.rs`:
`known_limitations_document_the_joint_ownership_entry_convention`,
`..._the_second_taxpayer_remedy`, `..._the_element_two_incidental_cost_convention`, and
`..._the_division_775_forex_omission`.
