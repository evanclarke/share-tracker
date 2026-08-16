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
