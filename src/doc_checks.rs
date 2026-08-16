//! Tests pinning documentation-only requirements (a TODO item is only done when
//! a test exists for it — CLAUDE.md). Each test asserts its required text —
//! typically a Known-limitations entry in `docs/API.md`, its README surfacing,
//! and the cited ATO mirror — is present, so the documented scope cut can't
//! silently vanish.

const API_MD: &str = include_str!("../docs/API.md");
const README_MD: &str = include_str!("../README.md");
const SCHEMA_MD: &str = include_str!("../docs/SCHEMA.md");
const DENY_TOML: &str = include_str!("../deny.toml");

/// The body of the `# Known limitations` section of `docs/API.md`.
fn known_limitations() -> &'static str {
    let section = API_MD
        .split("# Known limitations")
        .nth(1)
        .expect("docs/API.md has a Known limitations section");
    section
        .split("\n# ")
        .next()
        .expect("split always yields at least one part")
}

/// Docs-sync pin for the append-only audit trail (2026-07-13 improvement
/// review): the schema documents the table, its scope decision (which tables
/// are audited and why the rest are not), and the keep-forever retention
/// decision; the API documents the inspection endpoint; the README surfaces
/// the feature and cites the ATO record-keeping guidance it aligns with.
#[test]
fn row_history_audit_trail_documented() {
    // SCHEMA.md: the table and both recorded decisions.
    assert!(SCHEMA_MD.contains("row_history"));
    assert!(SCHEMA_MD.contains("**append-only audit trail**"));
    assert!(SCHEMA_MD.contains("scope decision 2026-07-14"));
    assert!(SCHEMA_MD.contains("Retention (decision 2026-07-14): entries are kept forever"));
    assert!(SCHEMA_MD.contains("deliberately has no pruning job"));
    // API.md: the inspection endpoint, its section, and its 422.
    assert!(API_MD.contains("### Row history"));
    assert!(API_MD.contains("POST /reports/row_history"));
    assert!(API_MD.contains("a row-history request naming a table that is not audited"));
    // README: the feature line and the ATO citation.
    assert!(README_MD.contains("**Append-only audit trail**"));
    assert!(README_MD.contains("docs/ato/cgt-keeping-records-shares.md"));
    assert!(
        include_str!("../docs/ato/cgt-keeping-records-shares.md")
            .contains("keeping-records-of-shares-and-units"),
        "the cited ATO mirror carries its source header"
    );
}

/// Docs-sync pin for linked attachments on provenance-created trades
/// (REQUIREMENTS 2026-07-15): the Attachments section documents the
/// `include_linked` list option, enumerates the three traversed provenance
/// links, and states that ownership is unchanged; the web-UI feature text
/// mentions linked documents; the 422 catalogue covers the option's misuse.
#[test]
fn linked_attachments_documented() {
    // The list-endpoint option and its linked-documents explainer.
    assert!(API_MD.contains("include_linked=true"));
    assert!(API_MD.contains("**Linked documents:**"));
    // All three provenance links are enumerated.
    assert!(API_MD.contains("`income.reinvestment_trade_id`"));
    assert!(API_MD.contains("`income.buyback_trade_id`"));
    assert!(API_MD.contains("`trades.ess_statement_id`"));
    // Attachments stay single-owner; the traversal is read-time only.
    assert!(API_MD.contains("Ownership is unchanged"));
    // The web-UI feature text mentions linked documents.
    assert!(API_MD.contains("linked documents](#attachments)"));
    // The 422 catalogue covers misuse of the option.
    assert!(API_MD.contains("an attachment list combining `include_linked`"));
}

/// Docs-sync pin for provisional snapshots, the job catch-up windows, and
/// the bulk regeneration controls (REQUIREMENTS 2026-07-16): the schema
/// documents the `provisional` column, the API documents the flag in
/// responses, the two regeneration endpoints, the import true-up, the
/// valuation-only FX fallback rule, and the 422 wording; the README surfaces
/// the provisional-then-finalised behaviour and both self-healing windows.
#[test]
fn provisional_snapshots_and_catchup_documented() {
    // SCHEMA.md: the provisional column, distinct from stale.
    assert!(SCHEMA_MD.contains("provisional   INTEGER"));
    assert!(SCHEMA_MD.contains("used a fallback-month FX rate"));
    assert!(SCHEMA_MD.contains("Distinct from stale"));
    // API.md: the flag in list/series responses, the endpoints, the true-up,
    // and the fallback rule (valuation-only, bounded, never a tax figure).
    assert!(API_MD.contains("`stale`, `provisional`"));
    assert!(API_MD.contains("**Provisional snapshots:**"));
    assert!(API_MD.contains("/report_snapshots/regenerate_all"));
    assert!(API_MD.contains("/report_snapshots/regenerate_provisional"));
    assert!(API_MD.contains("snapshot_true_up"));
    assert!(API_MD.contains("**Valuation-only fallback:**"));
    assert!(API_MD.contains("No tax calculation or FY report can reach a fallback-month rate"));
    assert!(API_MD.contains("`fx_provisional: true`"));
    // The 422 catalogue reflects the bounded fallback.
    assert!(API_MD.contains("an FX-rate gap too old for the 2-month valuation fallback"));
    // README: provisional-then-finalised snapshots and the two lookbacks.
    assert!(README_MD.contains("flagged **provisional**"));
    assert!(README_MD.contains("finalised automatically"));
    assert!(README_MD.contains("self-heals the last 14 calendar days"));
    assert!(README_MD.contains("backfills missing dates over a 14-day window"));
}

/// Docs-sync pin for date-ranged bulk regeneration (REQUIREMENTS 2026-07-25):
/// the API documents the new default-range endpoint, `regenerate_all`'s
/// range/backfill semantics and its 422, and the README surfaces that
/// regenerate-all is date-ranged and defaults to the whole history.
#[test]
fn regenerate_all_date_range_documented() {
    assert!(API_MD.contains("/report_snapshots/regenerate_range"));
    assert!(API_MD.contains("a backfill for dates that never had a snapshot"));
    assert!(API_MD.contains("clamped up to the first-ever-held date"));
    assert!(API_MD.contains("returns `422` if its resolved `from` is after its `to`"));
    assert!(README_MD.contains("date-ranged regenerate-all action"));
}

/// Docs-sync pin for the AMIT cash-only income rows (REQUIREMENTS
/// 2026-06-12): the Income section documents the cash-only rule and its 422s,
/// the Tax summary section documents the exclusion, the cross-check report
/// has its own section, and the README surfaces both features.
#[test]
fn amit_cash_only_rows_documented() {
    // Income section: the cash-only rule and the write-time validation.
    assert!(API_MD.contains("**AMIT cash distributions (cash-only rows):**"));
    assert!(API_MD.contains("the AMMA statement's attribution is the only assessable record"));
    // Tax summary section: the whole-row exclusion.
    assert!(API_MD.contains("**AMIT listing are excluded entirely**"));
    // The cross-check report's own section.
    assert!(API_MD.contains("### AMIT cash cross-check"));
    assert!(API_MD.contains("GET /reports/amit_cash_cross_check"));
    // README features.
    assert!(README_MD.contains("cash-only income rows"));
    assert!(README_MD.contains("**AMIT cash cross-check**"));
}

/// Docs-sync pin for AMIT adjustment cross-check and generation
/// (REQUIREMENTS 2026-08-13): the API documents the generation endpoint with
/// each of its refusals, the cross-check report, the write-time duplicate
/// rejection, and the annual tax report's now-four-part completeness; the
/// schema records the UNIQUE index that backs the invariant; the README
/// surfaces the feature and the completeness wording.
#[test]
fn amit_adjustment_generation_and_cross_check_documented() {
    // The generation endpoint, its reconciliation-not-invariant stance, and
    // its preview mode.
    assert!(API_MD.contains("### Generating AMIT adjustments"));
    assert!(API_MD.contains("POST /amma_statements/:id/generate_adjustments"));
    assert!(API_MD.contains("does not block the write"));
    assert!(API_MD.contains(r#"`"preview": true` computes the same result and writes nothing"#));
    // Each 422 refusal.
    assert!(API_MD.contains("**already has adjustments**"));
    assert!(API_MD.contains("**no parcels** of the statement's listing were open"));
    // A split across the covered parcels is *not* a refusal (SCENARIOS
    // B-24): each parcel's stored as-acquired quantity is re-based into the
    // statement year's basis before the per-unit figure is applied.
    assert!(API_MD.contains("**share split** between the covered parcels' acquisition dates"));
    assert!(API_MD.contains("re-based into the year-end basis × `cost_base_adjustment`"));
    // The write-time duplicate invariant and the index behind it.
    assert!(
        API_MD.contains("**another row already adjusts the same parcel on the same statement**")
    );
    assert!(SCHEMA_MD.contains("UNIQUE (amma_statement_id, trade_id)"));
    // The cross-check report's own section, with each of its four checks.
    assert!(API_MD.contains("### AMIT adjustment cross-check"));
    assert!(API_MD.contains("GET /reports/amit_adjustment_cross_check"));
    for check in [
        "**no adjustments at all**",
        "**coverage mismatch**",
        "**duplicate parcel**",
        "**parcel outside the statement's year**",
    ] {
        assert!(API_MD.contains(check), "missing cross-check bullet {check}");
    }
    // The annual tax report's completeness gate is now four lists, and stays
    // non-blocking.
    assert!(API_MD.contains("`amit_adjustment_alerts`"));
    assert!(API_MD.contains("`complete` is true only when all four are empty"));
    assert!(API_MD.contains("**`completeness`** — non-blocking (never rejects the request)"));
    // README: the feature line and the completeness wording.
    assert!(README_MD.contains("**AMIT adjustment generation and cross-check**"));
    assert!(README_MD.contains("per-parcel AMIT adjustments reconcile to it"));
}

/// Docs-sync pin for SCENARIOS F-05: which parcels a statement's per-unit
/// `cost_base_adjustment` reaches is a stated convention, not an unwritten
/// one — a parcel bought after the fund's last distribution period is covered
/// like any other, a statement quoting a *total* is divided over the units it
/// covers, and a member wanting another apportionment enters the rows by
/// hand. The ATO mirror it cites states the amount annually and per member.
#[test]
fn the_per_unit_apportionment_across_parcels_is_documented() {
    assert!(API_MD.contains("**Which parcels the per-unit figure reaches.**"));
    assert!(API_MD.contains("**uniformly to every unit held at the statement's"));
    assert!(API_MD.contains("bought *after* the fund's last distribution period"));
    assert!(API_MD.contains("dividing that total over the units the statement covers"));
    // The cited ATO mirror does state it as an annual, member-level amount.
    assert!(
        include_str!("../docs/ato/amit-cost-base-adjustments.md")
            .contains("AMIT cost base net amount for the income year in relation to your units")
    );
}

/// Docs-sync pin for SCENARIOS F-03/F-08: AMMA coverage is asked per holding
/// account, in both the cash cross-check and the annual tax report's
/// completeness section — a registry issues one statement per holder account.
#[test]
fn amma_coverage_is_documented_as_per_holding_account() {
    assert!(API_MD.contains("Coverage is asked **per holding account**"));
    assert!(API_MD.contains("one record per affected (listing, year, account) triple"));
    assert!(API_MD.contains("It is asked **per holding account** (each row carries its"));
    assert!(README_MD.contains("per holding account"));
}

#[test]
fn known_limitations_document_gifts_at_market_value() {
    let limitations = known_limitations();
    // Gifts / off-market related-party transfers: a disposal at market value
    // (market-value substitution), entered as a manual Sell or Buy at market value.
    assert!(limitations.contains("**Gifts / off-market related-party transfers**"));
    assert!(limitations.contains("market-value substitution rule"));
    assert!(limitations.contains("manual Sell at market-value proceeds"));
    assert!(limitations.contains("manual Buy at market-value cost"));
    // Cites the mirrored ATO guidance (QC 66021).
    assert!(limitations.contains("docs/ato/capital-proceeds-market-value-substitution.md"));
    assert!(
        include_str!("../docs/ato/capital-proceeds-market-value-substitution.md")
            .contains("QC 66021")
    );
    // Surfaced in the README too.
    assert!(README_MD.contains("gifts / off-market related-party transfers"));
    assert!(README_MD.contains("market-value substitution"));
}

#[test]
fn known_limitations_document_pre_cgt_holdings() {
    let limitations = known_limitations();
    // Pre-CGT holdings (acquired before 20 September 1985) are outside CGT and
    // not modelled — and since 2026-07-13 the limitation is enforced at write
    // time: a pre-CGT-dated trade (or an inheritance from a pre-CGT death) is
    // rejected 422 rather than wrongly computing gains on the parcel.
    assert!(limitations.contains("**Pre-CGT holdings**"));
    assert!(limitations.contains("before **20 September 1985** is outside CGT"));
    assert!(limitations.contains("pre-CGT holdings are not modelled"));
    assert!(limitations.contains("enforced at write time 2026-07-13"));
    assert!(limitations.contains("**cannot be entered**"));
    assert!(README_MD.contains("pre-CGT holdings"));
    assert!(README_MD.contains("acquired before 20 September 1985"));
    assert!(README_MD.contains("rejected at write time"));
}

/// Known-limitation pin (REQUIREMENTS 2026-06-12): dividend equivalents on
/// unvested RSU grants are ordinary income when paid (TD 2017/26) and are not
/// modelled — enterable manually as income if paid out in cash.
#[test]
fn known_limitations_document_rsu_dividend_equivalents() {
    let limitations = known_limitations();
    assert!(limitations.contains("**RSU dividend equivalents**"));
    assert!(limitations.contains("dividend equivalents on unvested RSU grants"));
    assert!(limitations.contains("**ordinary income when paid**"));
    assert!(
        limitations.contains("paid out in cash is enterable manually as an [income](#income) row")
    );
    // Cites the mirrored ATO ruling (TD 2017/26).
    assert!(limitations.contains("docs/ato/ess-dividend-equivalents.md"));
    assert!(include_str!("../docs/ato/ess-dividend-equivalents.md").contains("TD 2017/26"));
    // Surfaced in the README too.
    assert!(README_MD.contains("dividend equivalents on unvested RSU grants"));
    assert!(README_MD.contains("ordinary income when paid"));
}

/// Known-limitation pin (REQUIREMENTS "Ticker and exchange-code changes",
/// 2026-07-26; narrowed 2026-07-28 to settlement only, once price collection
/// started resolving its symbol and calendar as at the date fetched): an
/// exchange change recorded via `POST /listings/:id/rename` doesn't
/// retroactively pin historical trades to the calendar in force at the time —
/// re-saving a trade dated before the change without an explicit
/// `settlement_date` recomputes it against the listing's *current* exchange.
#[test]
fn known_limitations_document_exchange_change_recomputation() {
    let limitations = known_limitations();
    assert!(limitations.contains(
        "Settlement dates follow the listing's *current* exchange, not the date of the change"
    ));
    assert!(limitations.contains("settlement-holiday calendar"));
    assert!(limitations.contains("`trades.settlement_date` is a stored column"));
    assert!(limitations.contains("recomputes it against the exchange currently on the listing"));
    // The price path is explicitly carved out — it is no longer an instance
    // of this limitation.
    assert!(limitations.contains("no longer shares this limitation"));
}

/// Price collection resolves the provider symbol and the trading calendar as
/// at the date being fetched, from the rename chain (2026-07-28) — the fix
/// that made the fetch and snapshot-valuation paths agree across a rename.
#[test]
fn as_at_price_symbol_resolution_documented() {
    assert!(API_MD.contains("The symbol is resolved **as at the date being fetched**"));
    assert!(API_MD.contains("one provider call per identity"));
    // `price_symbol` is the current spelling, so it must not relabel history.
    assert!(API_MD.contains("applies only to dates in the listing's current identity"));
    // The collection window and the snapshot catch-up window are one length.
    assert!(API_MD.contains("same length as the [report-snapshot](#report-snapshots) catch-up"));
    assert!(README_MD.contains("under the symbol in force on each date"));
}

/// Known-limitation pin (REQUIREMENTS "Ticker and exchange-code changes",
/// 2026-07-26): a rename carries no staleness trigger for `listing_renames`
/// (the ticker is a display label over `listing_id`, never a computed
/// figure), so a snapshot generated before a rename keeps its pre-rename
/// ticker label until regenerated.
#[test]
fn known_limitations_document_snapshot_ticker_labels_are_display_only() {
    let limitations = known_limitations();
    assert!(limitations.contains("Snapshot ticker labels are display-only"));
    assert!(limitations.contains("`listing_renames` deliberately carries no staleness trigger"));
    assert!(limitations.contains("This is display drift only"));
}

/// Ticker/exchange renames (REQUIREMENTS "Ticker and exchange-code changes",
/// 2026-07-26): a rename is an explicit, dated, audited event once a listing
/// has history — the rename action, the provider-symbol override, and the
/// as-at ticker resolution used by the tax report and activity ledger are
/// all documented in the Listings section, and the feature is in README.
#[test]
fn listing_rename_action_documented() {
    assert!(API_MD.contains("POST /listings/:id/rename"));
    assert!(API_MD.contains("GET /listings/:id/renames"));
    assert!(API_MD.contains("DELETE /listings/:id/renames/:rename_id"));
    assert!(API_MD.contains("price_symbol"));
    assert!(API_MD.contains("resolves **as at its own date**"));
    assert!(README_MD.contains("Ticker and exchange-code renames"));
}

/// Resolved-limitation pin (REQUIREMENTS 2026-07-13; formerly the
/// "Foreign broker-cash interest classification" Known limitation, 2026-06-12):
/// interest income is classified by payer — Australian-source at question 10
/// (10L), foreign-source (broker-cash/money-market income) at 20E assessable
/// foreign source income with its foreign tax joining the FITO — and the
/// limitation entry is gone from Known limitations.
#[test]
fn docs_document_foreign_interest_source_classification() {
    // The limitation no longer exists — the classification is a feature.
    let limitations = known_limitations();
    assert!(!limitations.contains("**Foreign broker-cash interest classification**"));
    // The interest-income entity section documents the 20E routing…
    assert!(API_MD.contains(
        "instead reports as assessable foreign source income on the `foreign_interest_income` \
         line (question 20, label 20E)"
    ));
    // …and the label mapping carries the foreign interest column at 20E.
    assert!(API_MD.contains("| `interest_income` | `10L` | Australian gross interest"));
    assert!(API_MD.contains(
        "| `foreign_interest_income`, `foreign_source_income`, `amma_foreign_income` | `20E + 20M`"
    ));
    // The mirrored label reference carries both labels.
    let labels = include_str!("../docs/ato/tax-return-labels-2026.md");
    assert!(labels.contains("| 10L | Gross interest"));
    assert!(labels.contains("| 20E | Assessable foreign source income"));
    // Surfaced in the README too.
    assert!(README_MD.contains("**foreign-source** row"));
    assert!(README_MD.contains("20E assessable foreign source income"));
}

/// Docs-sync pin for the collectables / personal-use-asset scope decision
/// (2026-07-29, QC 106842): their capital losses are *quarantined* — a
/// collectable's loss can only reduce a gain from another collectable, and a
/// personal-use asset's loss is disregarded entirely — but this system has one
/// loss pool and no asset-class dimension, so entering one would wrongly
/// offset share gains. That has to be stated, not just omitted, because the
/// failure is silent: the net capital gain simply comes out too low. This is
/// the limitation the Kathleen acceptance test's jewellery leg cites for
/// staying out (`src/ato_examples.rs`).
#[test]
fn known_limitations_document_quarantined_collectable_losses() {
    let limitations = known_limitations();
    assert!(limitations.contains("**Collectables and personal-use assets"));
    assert!(limitations.contains("quarantined"));
    // The two distinct rules, and why entering one here would be wrong.
    assert!(limitations.contains("reduce a capital gain from another collectable"));
    assert!(limitations.contains("never a gain on shares"));
    assert!(limitations.contains("disregarded entirely"));
    assert!(limitations.contains("wrongly offset share gains"));
    // Cites the mirrored ATO source carrying the Kathleen examples.
    assert!(limitations.contains("docs/ato/capital-gains-question-18.md"));
    let mirror = include_str!("../docs/ato/capital-gains-question-18.md");
    assert!(mirror.contains("QC 106842"));
    // The mirror states the step order the net-capital-gain report implements.
    assert!(mirror.contains("before applying the CGT discount"));
    assert!(mirror.contains("$1,260"));
}

#[test]
fn known_limitations_document_indexation_method() {
    let limitations = known_limitations();
    // The indexation method (pre-21 September 1999 acquisitions, frozen at
    // Sep 1999) is not modelled; the 50% discount is used throughout.
    assert!(limitations.contains("**Indexation method**"));
    assert!(limitations.contains("before **21 September 1999**"));
    assert!(limitations.contains("frozen at the 30 September 1999 CPI"));
    assert!(limitations.contains("indexation is not modelled"));
    assert!(limitations.contains("50% discount is used throughout"));
    // Cites the mirrored ATO guidance (QC 66024).
    assert!(limitations.contains("docs/ato/indexing-the-cost-base.md"));
    assert!(include_str!("../docs/ato/indexing-the-cost-base.md").contains("QC 66024"));
    assert!(README_MD.contains("indexation method"));
    assert!(README_MD.contains("50% discount is used throughout"));
}

/// Docs-sync pin for the FX spot-rate override (REQUIREMENTS 2026-06-12, QC
/// 18020): the FX conversion section states the precedence rule honestly —
/// spot override first, monthly RBA rate as the ATO-published convenience
/// default, `fx_rate` fallback, loud failure — and says when the ATO expects
/// a spot rate; the Trades section documents the field and its 422s; the
/// README surfaces the override; and the cited ATO mirrors carry their QC
/// headers.
#[test]
fn fx_spot_rate_override_documented() {
    // The FX conversion section: full precedence and the honesty note.
    assert!(API_MD.contains("**`spot_fx_rate` override**"));
    assert!(API_MD.contains("monthly rate is the ATO-published convenience default"));
    assert!(
        API_MD.contains(
            "**not appropriate for a one-off purchase or sale of a large capital asset**"
        )
    );
    assert!(API_MD.contains("used only when no ATO rate has been imported"));
    // The Trades section: deliberate entry, the 422s, and the rollover carry.
    assert!(
        API_MD
            .contains("rejected with `422` when non-positive or supplied on an AUD-currency trade")
    );
    assert!(API_MD.contains("carry a consumed parcel's override onto its replacement Buys"));
    // Cited mirrors carry their QC headers.
    assert!(API_MD.contains("docs/ato/forex-average-rates.md"));
    assert!(include_str!("../docs/ato/forex-average-rates.md").contains("QC 18020"));
    assert!(API_MD.contains("docs/ato/forex-common-transactions.md"));
    assert!(include_str!("../docs/ato/forex-common-transactions.md").contains("QC 18322"));
    // README features bullet.
    assert!(README_MD.contains("transaction-date spot-rate override"));
    assert!(README_MD.contains("QC 18020"));
}

/// Doc-only resolution pin for cost-base FX timing (2026-07-12 review,
/// decided 2026-07-13 as a documented limitation): `CostBase::into_aud_with`
/// converts the whole breakdown — including AMIT (E10) and return-of-capital
/// (G1) reductions from later rate months — at the parcel's acquisition-month
/// rate, a deliberate simplification of the s 960-50(6) per-transaction
/// translation timing. The Known-limitations entry states the rule, the
/// g1_gains asymmetry (a payment's excess converts at the payment month while
/// its reduction converts at the acquisition month), and when it bites (a
/// non-AUD holding with non-AUD reductions — none in practice); cites the
/// QC 18322 mirror; is cross-linked from the FX-conversion section; and the
/// README surfaces it.
#[test]
fn known_limitations_document_cost_base_fx_timing() {
    let limitations = known_limitations();
    assert!(limitations.contains(
        "**Cost-base FX timing — AMIT/return-of-capital reductions convert at the \
         acquisition-month rate**"
    ));
    assert!(limitations.contains("**one rate: the parcel's (possibly deemed) acquisition month**"));
    // The strict rule and its citation.
    assert!(limitations.contains("s 960-50(6) translation rules"));
    assert!(limitations.contains("docs/ato/forex-common-transactions.md"));
    assert!(include_str!("../docs/ato/forex-common-transactions.md").contains("QC 18322"));
    // The g1_gains asymmetry, both halves.
    assert!(limitations.contains("converts at the **payment month**"));
    assert!(limitations.contains("converts at the **acquisition month**"));
    // When it bites.
    assert!(
        limitations
            .contains("**non-AUD holding receiving non-AUD AMIT/return-of-capital reductions**")
    );
    // Cross-linked from the FX-conversion section.
    assert!(API_MD.contains(
        "including the AMIT/return-of-capital reductions that arose in later rate months \
         (see [Known limitations](#known-limitations))"
    ));
    // Surfaced in the README too.
    assert!(README_MD.contains("cost-base reductions convert at the acquisition-month rate"));
    assert!(README_MD.contains("s 960-50 translation timing"));
}

/// Docs pin for the brokerage-currency invariant (SCENARIOS B-02, decided
/// 2026-08-15): the write-time refusal has its own tests
/// (`trade::tests::api_brokerage_in_another_currency_than_the_trade_returns_422`,
/// the Sell twin, and the open-parcels figure), but the *documented* half —
/// what a user with a foreign-currency fee is supposed to do instead, and why
/// the strict s 960-50 per-leg translation is not modelled — lives only in the
/// docs. The Known-limitations entry states the rule, the workaround and the
/// citation; the Trades section explains what the field means for the cost
/// base; the schema records the constraint; and the README surfaces it.
#[test]
fn known_limitations_document_the_brokerage_currency_invariant() {
    let limitations = known_limitations();
    assert!(limitations.contains("**Brokerage is billed in the trade's own currency**"));
    // The rule, the reason, and the entry route it leaves the user.
    assert!(limitations.contains("`brokerage_currency` differs from its `currency` is rejected"));
    assert!(limitations.contains(
        "**An Australian broker's AUD commission on a US trade is entered converted into the \
         trade's currency**"
    ));
    // The strict rule that is deliberately not modelled, and its mirror.
    assert!(limitations.contains("s 960-50 translates each amount at its own time"));
    assert!(limitations.contains("docs/ato/forex-common-transactions.md"));
    // The Trades section says what the field means for the cost base.
    assert!(API_MD.contains(
        "`brokerage_currency` records the currency the fee was billed in, and **must equal the \
         trade's `currency`**"
    ));
    // The schema carries the constraint on the column itself.
    assert!(SCHEMA_MD.contains("write-time validated to equal `currency` (422 otherwise)"));
    // Surfaced in the README too.
    assert!(README_MD.contains("**brokerage is billed in the trade's own currency**"));
}

/// Docs pin for the return-of-capital currency invariant (SCENARIOS E-07 /
/// E-39, fixed 2026-08-16): both write-time refusals have their own tests
/// (`corporate_action::tests::api_payment_in_another_currency_than_its_parcels_returns_422`
/// and the parcel-side twin in `trade::tests`), but the *documented* half —
/// which parcels the check covers, that the two entry paths refuse the pair
/// from either side, and the one residual route into the mismatch (a rollover
/// carrying a parcel's own currency onto a listing that already has a
/// differing payment) — lives only in the docs.
#[test]
fn return_of_capital_currency_invariant_documented() {
    // The rule and its scope, in the Corporate actions section.
    assert!(API_MD.contains(
        "A `ReturnOfCapital`'s `currency` must match the currency of the parcels it reduces"
    ));
    assert!(API_MD.contains("a differing pair is rejected with `422` naming both currencies"));
    assert!(API_MD.contains(
        "those acquired before its `record_date`, or on or before the payment date when none is \
         recorded"
    ));
    // A rollover-created parcel keeps the original's currency, so the listed
    // currency is the wrong one to reach for — and that is the residual route
    // into the mismatch the reports still fail loudly on.
    assert!(
        API_MD.contains("record its return of capital in the parcels' currency, not the listing's")
    );
    assert!(API_MD.contains(
        "a replacement parcel created *after* a differing payment was recorded is the one \
         remaining way to meet the mismatch"
    ));
    // The parcel side of the same rejection, in the Trades section.
    assert!(API_MD.contains(
        "`PUT /trades/:id` returns `422` if a Buy/DRP's `currency` differs from that of a \
         [return of capital](#corporate-actions) recorded on its listing that reaches it"
    ));
    // The schema carries the constraint on the column itself.
    assert!(SCHEMA_MD.contains(
        "write-time validated to equal the currency of the parcels the payment reaches (422 \
         otherwise)"
    ));
}

/// Docs-sync pin for the backup pipeline hardening (2026-07-13 improvement
/// review) and the later `--backup-command` off-machine-copy hook: the README
/// documents verification (integrity check + migrations match, `.bad`
/// quarantine), the retention policy (newest 8 + 12 monthly keepers,
/// pattern-matched files only), and — the recorded decision for the
/// off-machine copy — that the server never embeds remote credentials or
/// provider-specific upload logic itself, only ever shelling out to an
/// operator-configured command (`--backup-command`) or leaving it to an
/// independent cron job; the jobs API documents the backup job's failure
/// semantics.
#[test]
fn backup_pipeline_documented() {
    // Verification + quarantine.
    assert!(README_MD.contains("must pass `PRAGMA integrity_check`"));
    assert!(README_MD.contains("applied migrations must match the live database's"));
    assert!(README_MD.contains("quarantined by renaming it to `<name>.db.bad`"));
    assert!(README_MD.contains("never left looking like a good backup"));
    // Retention policy, and its only-pattern-matched-files deletion scope.
    assert!(README_MD.contains("**newest 8 backups**"));
    assert!(
        README_MD.contains("**first backup of each calendar month for the 12 most recent months**")
    );
    assert!(README_MD.contains(
        "Pruning deletes only files matching the backup filename pattern for this database"
    ));
    // The off-machine copy decision: no embedded credentials/provider logic —
    // either the `--backup-command` hook or an independent cron job, both
    // driven entirely by operator-supplied configuration.
    assert!(README_MD.contains("### Off-machine copies"));
    assert!(
        README_MD
            .contains("never embeds remote credentials or provider-specific upload logic itself")
    );
    assert!(README_MD.contains("--backup-command"));
    assert!(README_MD.contains("{BACKUP_FILE}"));
    assert!(README_MD.contains("rclone sync /mnt/backups"));
    // The jobs API documents the backup job's verify/prune/failure semantics.
    assert!(API_MD.contains("`POST /jobs/backup` returns `500`"));
    assert!(API_MD.contains("quarantined as `<name>.db.bad`"));
    assert!(API_MD.contains(
        "the newest 8 backups plus the first backup of each of the 12 most recent months"
    ));
}

/// Docs-sync pin for the `?suffix=` param on `POST /jobs/:name` (the
/// update.sh pre-upgrade backup): the jobs API documents its allowed
/// characters and the `422` it produces when invalid, that a suffixed backup
/// is pruned by the same retention policy as any other, and the README
/// documents update.sh's abort-before-`pkg add` behaviour and the
/// `-n`/`--no-backup` escape hatch.
#[test]
fn backup_suffix_param_documented() {
    assert!(API_MD.contains("`POST /jobs/:name?suffix=`"));
    assert!(API_MD.contains(
        "Allowed characters are ASCII letters, digits, `.`, `_`, and `-`, up to 40 characters"
    ));
    assert!(API_MD.contains(
        "A suffixed one-off backup is pruned by the same policy as any other — it is not exempt."
    ));
    assert!(README_MD.contains("aborts before touching the package"));
    assert!(README_MD.contains("`-n`/`--no-backup`"));
    assert!(README_MD.contains("never exempt from pruning"));
}

/// Doc-only resolution pin for settlement-window forex — CGT events K10/K11
/// (REQUIREMENTS 2026-06-12, NEEDS DECISION resolved 2026-06-12 as out of
/// scope): the Known-limitations entry states the rule (cost-base adjustment
/// on an acquisition, non-discountable K10 gain / K11 loss on a disposal),
/// that outcomes are the taxpayer's manual adjustment, and the interaction
/// with the spot-rate override (nil by construction under same-rate-month
/// monthly rates; per-leg spot rates make the movement visible); cites the
/// QC 17062 mirror; and the README surfaces it.
#[test]
fn known_limitations_document_settlement_window_forex_k10_k11() {
    let limitations = known_limitations();
    assert!(
        limitations.contains(
            "**Settlement-window forex on foreign-currency trades — CGT events K10/K11**"
        )
    );
    assert!(
        limitations.contains(
            "non-discountable capital gain (CGT event K10) or capital loss (CGT event K11)"
        )
    );
    assert!(limitations.contains("the taxpayer's manual adjustment"));
    // The spot-override interaction, both halves.
    assert!(limitations.contains("nil by construction"));
    assert!(limitations.contains("per-leg spot rates"));
    // Cites the mirrored ATO guidance (QC 17062) and its worked examples.
    assert!(limitations.contains("docs/ato/forex-cgt-12-month-rule.md"));
    assert!(limitations.contains("Art Ltd"));
    assert!(limitations.contains("Eleanor"));
    let mirror = include_str!("../docs/ato/forex-cgt-12-month-rule.md");
    assert!(mirror.contains("QC 17062"));
    // Surfaced in the README too.
    assert!(README_MD.contains("CGT events K10/K11"));
}

/// Docs-sync pin for hand-entered closing prices (2026-07-28): the API
/// documents the endpoint, both required provenance fields, and its 422s; the
/// schema documents the three columns and why the table still carries no
/// INSERT/DELETE staleness trigger and stays outside the audit trail; and the
/// Known-limitations entry states the two consequences the user lives with —
/// a manual price is one-way, and its superseded provenance is not retained.
#[test]
fn manual_closing_prices_documented() {
    // API.md: the endpoint, its body, and the one-way rule.
    assert!(API_MD.contains("PUT` | `/closing_prices/:listing_id/:price_date"));
    assert!(API_MD.contains("`sourced_from` (where the figure came from"));
    assert!(API_MD.contains("`reason` (why manual entry was needed"));
    assert!(API_MD.contains("**only ever replaced by another manual price**"));
    assert!(API_MD.contains("re-fetching a day whose stored price was entered manually"));
    // SCHEMA.md: the columns, the trigger reasoning, and the audit exclusion.
    assert!(SCHEMA_MD.contains("fetched | manual (CHECK-enforced enum, 0020)"));
    assert!(SCHEMA_MD.contains("Where a manual price was taken from"));
    assert!(SCHEMA_MD.contains("Why manual entry was needed"));
    assert!(SCHEMA_MD.contains("A **manually entered** price (0020) changes neither premise"));
    // Known limitations: the one-way rule, and that overwriting still loses
    // nothing now that the table is audited (0021).
    let limitations = known_limitations();
    assert!(limitations.contains("**A manually entered price is one-way**"));
    assert!(limitations.contains("cannot be removed — only overwritten"));
    assert!(limitations.contains("Overwriting loses nothing"));
    // README surfaces the feature.
    assert!(README_MD.contains("**priced by hand**"));
}

/// Docs-sync pin for auditing closing prices (2026-07-28): the schema records
/// why the table joined the audited set, the surrogate key it needed, and what
/// the old composite key became; the API documents the `id` and points a
/// history lookup at it; the README names hand-entered prices among the
/// audited facts.
#[test]
fn audited_closing_prices_documented() {
    // SCHEMA.md: the reversal of the original exclusion and the key change.
    assert!(SCHEMA_MD.contains("`closing_prices` joined the audited set in 0021"));
    assert!(SCHEMA_MD.contains("AUTOINCREMENT surrogate `id`"));
    assert!(SCHEMA_MD.contains("kept as `UNIQUE(listing_id, price_date)`"));
    assert!(SCHEMA_MD.contains("Server-assigned surrogate key"));
    // API.md: the id, what it is for, and that it survives a replacement.
    assert!(API_MD.contains("`{\"table\": \"closing_prices\", \"row_id\": <id>}`"));
    assert!(API_MD.contains("one row's whole revision history sits under one id"));
    assert!(API_MD.contains("Its `id` is the `row_id` to ask for."));
    // README: hand-entered prices are named among the audited facts.
    assert!(README_MD.contains("hand-entered closing prices"));
}

/// Docs-sync pin (REQUIREMENTS 2026-07-13, lossless GST-inclusive
/// round-trip): the Trades section states the read/write round-trip
/// semantics explicitly — with the flag set, `brokerage` is the one
/// GST-inclusive amount on both reads and writes, and a verbatim GET → PUT
/// re-splits to the identical stored pair — and the Sells section carries
/// the same contract for flagged Sells.
#[test]
fn gst_inclusive_round_trip_semantics_documented() {
    assert!(
        API_MD.contains("**Reads present the same shape writes expect (lossless round-trip):**")
    );
    assert!(API_MD.contains("**on both reads and writes**"));
    assert!(API_MD.contains("re-splits the same amount to the **identical stored pair**"));
    // The Sells section states it for flagged Sells as well.
    assert!(API_MD.contains("and the lossless GST-inclusive round-trip"));
    assert!(API_MD.contains("re-`PUT`ting that body to `PUT /sells/:id`"));
}

/// Pins the frontend test strategy (2026-07-13 improvement review): the JS
/// unit tests and the headless UI smoke check are CI steps, not manual-only
/// tools, and the README documents how to run them and the required Node
/// version. The exclusion of `*.test.js` files from the served bundle is
/// pinned in `web.rs` (`js_test_files_are_not_served_and_every_module_is`).
#[test]
fn frontend_tests_run_in_ci() {
    const CI_YML: &str = include_str!("../.github/workflows/ci.yml");
    // The Node unit-test step, on a pinned Node version.
    assert!(CI_YML.contains("node --test 'src/web/*.test.js'"));
    assert!(CI_YML.contains("node-version: '22'"));
    // The headless smoke-check step.
    assert!(CI_YML.contains("scripts/ui-smoke.sh"));
    // README: how to run them, and the Node version requirement.
    assert!(README_MD.contains("### Tests"));
    assert!(README_MD.contains("node --test 'src/web/*.test.js'"));
    assert!(README_MD.contains("**Node 22 or newer**"));
    assert!(README_MD.contains("scripts/ui-smoke.sh"));
}

/// Pins the supply-chain checks (2026-07-13 improvement review): the RustSec
/// advisory gate (`cargo deny check advisories`) is a CI step driven by the
/// committed `deny.toml`, Dependabot keeps the Cargo and GitHub Actions
/// dependencies patched with weekly grouped PRs, and the README documents the
/// local equivalent and the recorded no-upstream-fix policy.
#[test]
fn supply_chain_checks_run_in_ci() {
    const CI_YML: &str = include_str!("../.github/workflows/ci.yml");
    assert!(CI_YML.contains("EmbarkStudios/cargo-deny-action"));
    assert!(CI_YML.contains("command: check advisories"));
    // Dependabot: weekly grouped version updates for both ecosystems.
    const DEPENDABOT_YML: &str = include_str!("../.github/dependabot.yml");
    assert!(DEPENDABOT_YML.contains("package-ecosystem: cargo"));
    assert!(DEPENDABOT_YML.contains("package-ecosystem: github-actions"));
    assert!(DEPENDABOT_YML.contains("interval: weekly"));
    assert!(DEPENDABOT_YML.contains("groups:"));
    // deny.toml: the advisories config and the recorded policy.
    assert!(DENY_TOML.contains("[advisories]"));
    assert!(DENY_TOML.contains("Policy for an advisory with no upstream fix"));
    assert!(DENY_TOML.contains("temporary by construction, never"));
    // README: the section, the local equivalent, and the policy decision.
    assert!(README_MD.contains("### Supply-chain checks"));
    assert!(README_MD.contains("cargo deny check advisories"));
    assert!(README_MD.contains("cargo install cargo-deny --locked"));
    assert!(README_MD.contains("**Policy for an advisory with no upstream fix yet**"));
    assert!(README_MD.contains("never permanent"));
}

/// Pins least-privilege `GITHUB_TOKEN` scopes on every workflow (CodeQL
/// `actions/missing-workflow-permissions`, resolved 2026-07-30): a workflow
/// with no `permissions:` block inherits the repository default, so a later
/// change to that repository setting would silently widen what a compromised
/// action could do. Each workflow therefore declares its own floor — `ci.yml`
/// only reads the repo, `release.yml` needs `contents: write` to publish the
/// release — and a new workflow must declare one too.
#[test]
fn workflows_declare_explicit_token_permissions() {
    const CI_YML: &str = include_str!("../.github/workflows/ci.yml");
    const RELEASE_YML: &str = include_str!("../.github/workflows/release.yml");
    // CI publishes nothing and comments on nothing: read-only is the floor.
    assert!(CI_YML.contains("permissions:\n  contents: read"));
    // Releases create a tag + release, so this one is deliberately broader.
    assert!(RELEASE_YML.contains("permissions:\n  contents: write"));
    // Any workflow added later declares its own scopes rather than inheriting.
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/.github/workflows");
    for entry in std::fs::read_dir(dir).expect("workflows dir is readable") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().is_none_or(|e| e != "yml" && e != "yaml") {
            continue;
        }
        let body = std::fs::read_to_string(&path).expect("workflow is readable");
        assert!(
            body.contains("\npermissions:"),
            "{} declares no top-level permissions: block",
            path.display()
        );
    }
}

/// Pins third-party actions to an immutable commit SHA (CodeQL
/// `actions/unpinned-tag`, resolved 2026-07-30): a tag is mutable, so `@v2`
/// runs whatever its owner later repoints it at, with the workflow's token.
/// GitHub's own `actions/*` are first-party and exempt, matching the rule.
/// Each pin carries a trailing `# <version>` comment — that is what Dependabot
/// reads to raise the bump, so pinning hardens the workflows without freezing
/// them at today's versions.
#[test]
fn third_party_actions_are_pinned_to_a_commit_sha() {
    const CI_YML: &str = include_str!("../.github/workflows/ci.yml");
    const RELEASE_YML: &str = include_str!("../.github/workflows/release.yml");
    let mut checked = 0;
    for (file, body) in [("ci.yml", CI_YML), ("release.yml", RELEASE_YML)] {
        for line in body.lines() {
            if line.trim_start().starts_with('#') {
                continue; // prose about pinning, not a step
            }
            let Some((_, spec)) = line.split_once("uses:") else {
                continue;
            };
            let spec = spec.trim();
            let (action, reference) = spec
                .split_once('@')
                .unwrap_or_else(|| panic!("{file}: `uses: {spec}` names no ref"));
            if action.starts_with("actions/") {
                continue;
            }
            // Mutability first: on a reverted pin that is the real complaint,
            // and a bare tag has no comment either, so checking the comment
            // first would report the lesser problem.
            let (sha, version) = match reference.split_once('#') {
                Some((sha, version)) => (sha.trim(), Some(version.trim())),
                None => (reference.trim(), None),
            };
            assert!(
                sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
                "{file}: `{action}` is pinned to `{sha}`, not a 40-character commit SHA — a tag is mutable",
            );
            let version = version.unwrap_or_else(|| {
                panic!("{file}: `{action}`'s pin carries no `# <version>` comment for Dependabot")
            });
            assert!(
                !version.is_empty(),
                "{file}: `{action}`'s pin has an empty version comment",
            );
            checked += 1;
        }
    }
    // The four that exist today; a new third-party action raises this.
    assert_eq!(checked, 4, "expected 4 pinned third-party actions");
}

/// Executable half of the deny.toml ignore policy (decided 2026-07-14): every
/// advisory ignore entry carries a RustSec id, a non-empty reason, and — on
/// the same line, per the format the file documents — an
/// `# expires: YYYY-MM-DD` comment whose date has not yet passed. Once an
/// expiry passes this test fails, so the entry must be re-justified with a
/// new date or removed: an ignore cannot become permanent by inattention.
#[test]
fn advisory_ignores_expire() {
    let value: toml::Value = toml::from_str(DENY_TOML).expect("deny.toml parses as TOML");
    let ignore = value["advisories"]["ignore"]
        .as_array()
        .expect("advisories.ignore is an array");
    for entry in ignore {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .expect("ignore entry has a string id");
        assert!(id.starts_with("RUSTSEC-"), "id {id:?} is a RustSec id");
        let reason = entry
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("ignore entry {id} carries no reason"));
        assert!(
            !reason.trim().is_empty(),
            "ignore entry {id} has an empty reason"
        );
    }
    // One entry per line (the documented format), so each entry's line-end
    // expiry comment is attributable; comment lines (the format example)
    // don't count.
    let entry_lines: Vec<&str> = DENY_TOML
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && l.contains("RUSTSEC-"))
        .collect();
    assert_eq!(
        entry_lines.len(),
        ignore.len(),
        "each advisories.ignore entry sits on its own line with its expiry comment"
    );
    let today = chrono::Local::now().date_naive();
    for line in entry_lines {
        let expires = line.split("# expires: ").nth(1).unwrap_or_else(|| {
            panic!("ignore entry lacks an `# expires: YYYY-MM-DD` comment: {line}")
        });
        let date = chrono::NaiveDate::parse_from_str(expires.trim(), "%Y-%m-%d")
            .unwrap_or_else(|e| panic!("expiry {expires:?} is not YYYY-MM-DD ({e}): {line}"));
        assert!(
            date >= today,
            "advisory ignore expired on {date}: {line}\nre-justify it with a new expiry or remove it"
        );
    }
}

/// Docs-sync pin for the overview performance panel (REQUIREMENTS
/// 2026-07-25): the API docs the new endpoint's `(from, to]`/FX-attribution
/// conventions and its Known-limitations entry, the response-codes catalogue
/// covers its 422s, and the README surfaces the panel on the overview screen
/// (and no longer claims the graph lives on the snapshots screen).
#[test]
fn period_performance_panel_documented() {
    assert!(API_MD.contains("### Period performance"));
    assert!(API_MD.contains("POST /portfolio/period-performance"));
    assert!(API_MD.contains("half-open `(from, to]`"));
    assert!(
        API_MD.contains("a period-performance request whose `from` is not strictly before `to`")
    );
    let limits = known_limitations();
    assert!(limits.contains("Period-performance FX attribution is approximate"));
    assert!(limits.contains("capital_growth + fx_movement + income"));
    assert!(README_MD.contains("period performance"));
    assert!(README_MD.contains("capital growth"));
    assert!(README_MD.contains("date-range presets"));
}

/// Docs-sync pin for the overview panel's 2Y/3Y presets, remembered range,
/// and the per-holding "hide no-activity holdings" checkbox (REQUIREMENTS
/// 2026-07-26): the README's preset list includes 2Y/3Y and the remembered-
/// range/hide-inactive behaviour, and the API docs' Period performance
/// section explains that `holdings` returns every holding unfiltered and the
/// UI hides all-zero ones by default.
#[test]
fn overview_range_presets_and_activity_filter_documented() {
    assert!(README_MD.contains("1M/3M/6M/1Y/2Y/3Y/FY-to-date/all"));
    assert!(README_MD.contains("last-picked preset is remembered across reloads"));
    assert!(README_MD.contains("hide holdings with no activity in this period"));
    assert!(API_MD.contains(
        "a row for **every** holding with any history up to either endpoint, including one fully closed well before `from`"
    ));
    assert!(API_MD.contains("hide holdings with no activity in this period"));
}

/// Docs-sync pin for the top menu bar + overview-first home screen
/// (REQUIREMENTS 2026-07-25): the API docs name the menu bar, its four menus,
/// `#/` as the home route, and the new `/static/nav.js` module; the README's
/// Web UI bullet describes the menu bar, and the Portfolio overview bullet
/// names the home screen and its shortcut buttons.
#[test]
fn top_menu_bar_documented() {
    assert!(API_MD.contains("/static/nav.js"));
    assert!(API_MD.contains("top menu bar"));
    for menu in ["Activity", "Reports", "Reference Data", "Jobs"] {
        assert!(API_MD.contains(menu), "API.md should name the {menu} menu");
    }
    assert!(API_MD.contains("the app's home screen"));
    assert!(README_MD.contains("top menu bar"));
    assert!(README_MD.contains("New trade/income/sell/transfer shortcut buttons"));
}

/// Docs-sync pin for the uniform DELETE 404 contract (2026-07-29 Rust review):
/// the Response-codes table states that a `DELETE` of a missing row answers
/// with a plain-text reason (a `GET` still answers with an empty body), and
/// the Error-bodies paragraph counts deletes among the reasoned 404s. The
/// behaviour itself is pinned by
/// `entities::tests::deleting_a_missing_row_is_404_naming_what_was_missing`.
#[test]
fn delete_404_reason_documented() {
    assert!(API_MD.contains("A `GET` of a missing row answers with an empty body"));
    assert!(API_MD.contains("every `DELETE` of a missing row"));
    assert!(API_MD.contains("no AMMA statement with that id"));
    assert!(API_MD.contains("`404`-with-a-cause — which includes every `DELETE` of a row"));
}

/// Docs-sync pin for the delete-time guard on the three read-time corporate
/// actions (SCENARIOS A-06/A-20/A-21, 2026-08-14): the Corporate actions
/// section states the guard per type and the direction each one runs in, the
/// Response-codes `422` row lists both refusals, and the deliberately
/// *unguarded* `PUT` — the correction path the guard would otherwise close —
/// is a Known limitations entry rather than silent behaviour. The refusals
/// themselves are pinned by `entities::corporate_action::tests`.
#[test]
fn corporate_action_delete_guard_documented() {
    assert!(API_MD.contains("**Deleting an action that is already depended on.**"));
    assert!(API_MD.contains("**any trade dated on or after** the action's `date`"));
    assert!(API_MD.contains("**any parcel acquired on or before** the payment `date`"));
    assert!(API_MD.contains("deleting a ShareSplit or BonusIssue whose listing has a trade"));
    assert!(API_MD.contains("deleting a ReturnOfCapital whose listing has a parcel"));
    // The edit path stays open by decision, so it is documented as such —
    // narrowed (2026-08-15) by the write-time state check below, which the
    // limitation now points at as the bound on what an edit can leave behind.
    let limitations = known_limitations();
    assert!(limitations.contains(
        "**Editing a split, bonus issue, or return of capital in place restates prior figures**"
    ));
    assert!(limitations.contains("the residual exposure is restatement of valid figures only"));
    assert!(limitations.contains("There is no lodged/closed-year concept in the data model"));
}

/// Docs-sync pin for the return-of-capital record date (SCENARIOS B-09,
/// 2026-08-15): entitlement is fixed at the record date, and the docs say so
/// — including what the *absence* of a record date falls back to, which is
/// the over-reduction the field exists to correct. The behaviour itself is
/// pinned by `entities::corporate_action::tests` and the report tests.
#[test]
fn return_of_capital_record_date_documented() {
    assert!(API_MD.contains("entitlement is fixed earlier, at the **record date**"));
    assert!(API_MD.contains("**Leaving `record_date` out keeps the older, coarser rule**"));
    assert!(API_MD.contains("over-reduces a parcel bought inside the record-to-payment window"));
    // The schema documents the column, its CHECK, and the same fallback.
    assert!(SCHEMA_MD.contains("record_date       TEXT (date, nullable)  ReturnOfCapital only"));
    assert!(SCHEMA_MD.contains("never after `date`"));
    assert!(SCHEMA_MD.contains("NULL = not recorded, and the payment date decides instead"));
}

/// Docs-sync pin for the sale-side incidental-costs convention (SCENARIOS
/// B-17, 2026-08-15). Netting a Sell's brokerage off `proceeds` rather than
/// adding it to `cost_base` gives the identical capital gain, so nothing is
/// wrong — but the two *reported components* differ from the ATO's own
/// presentation, and a user reconciling against a worksheet finds two figures
/// that don't match and a gain that does. Documented, with the worked figures
/// and the reason the convention is what it is.
#[test]
fn sale_side_incidental_costs_convention_documented() {
    assert!(API_MD.contains("**Where a Sell's brokerage and GST land.**"));
    assert!(API_MD.contains("**netted off `proceeds`**"));
    assert!(API_MD.contains("**The capital gain is identical either way**"));
    // The worked example both presentations are shown through.
    assert!(API_MD.contains("`proceeds: 1189.055` / `cost_base: 1010.945`"));
    assert!(API_MD.contains("$1,200.00 / $1,021.89"));
    // Why: the parcel's cost base must read the same before and after sale.
    assert!(API_MD.contains("doesn't move the moment it is sold"));
    // And the ATO's own definition of the second element.
    assert!(API_MD.contains("docs/ato/cgt-cost-base.md"));
    assert!(
        include_str!("../docs/ato/cgt-cost-base.md").contains(
            "Second element: incidental costs of acquiring the CGT asset or that relate to the \
             CGT event"
        ),
        "the cited ATO mirror still carries the second-element definition"
    );
}

/// Docs-sync pin for rights acquired beyond the holding's own entitlement
/// (SCENARIOS B-20, 2026-08-15): `rights_cost` reads as though purchased
/// rights were fully supported, while both endpoints cap cumulative units at
/// the record-date entitlement and refuse past it. The cap is a safe refusal,
/// so the gap is documentation — the exercise section and Known limitations
/// now say what is and isn't recordable, and where the extra shares go.
#[test]
fn rights_beyond_the_entitlement_documented() {
    // The exercise section: purchased rights are in scope only within the
    // entitlement, with the refusal quoted.
    assert!(API_MD.contains("`rights_cost` covers rights **bought on-market**"));
    assert!(API_MD.contains(
        "the units exercised exceed the entitlement earned by the holding at the record date"
    ));
    // Known limitations: the scope cut and the entry route it leaves.
    let limitations = known_limitations();
    assert!(limitations.contains("**rights acquired beyond the holding's own entitlement**"));
    assert!(limitations.contains("supported up to the entitlement the holding earned"));
    assert!(limitations.contains("a purchase of extra rights has nowhere to be recorded"));
    assert!(limitations.contains("as an ordinary [Buy](#trades) at their full acquisition cost"));
}

/// Docs-sync pin for the rollover-assumed scope cut (SCENARIOS C-09,
/// 2026-08-15). `ScripForScrip` and `Demerger` model only the rollover case,
/// and recording one *is* the assertion that the rollover applies — nothing
/// checks eligibility. The behaviour is right for what it models and the
/// no-rollover variant fails safe (there is no operation to invoke; the user
/// enters the trades by hand), so the gap was that "not modelled" lived only
/// in the per-action prose and `docs/ato/demergers.md` — never in the Known
/// limitations list a reader scans for scope cuts, unlike the neighbouring
/// rights-issue entry. It matters because the two cases differ in the
/// *opposite* direction on the discount clock: with rollover the new
/// interests carry the original acquisition date, without it they start
/// their own at the event date.
#[test]
fn rollover_assumed_scope_cut_documented() {
    let limitations = known_limitations();
    assert!(limitations.contains("**Rollovers assume the rollover was chosen**"));
    // Recording the action is the assertion; nothing verifies eligibility.
    assert!(limitations.contains("the taxpayer's assertion that the rollover applies"));
    assert!(limitations.contains("nothing checks eligibility"));
    // The two directions of the discount clock, which is what C-09 turns on.
    assert!(limitations.contains("`deemed_acquisition_date`"));
    assert!(limitations.contains("run their own 12-month clock from it"));
    // The manual entry route each no-rollover case leaves.
    assert!(limitations.contains("enter a no-rollover exchange as a manual Sell plus Buy"));
    assert!(limitations.contains("dated the demerger date"));
    // The ATO mirror cited, still carrying the example the rule comes from.
    assert!(limitations.contains("docs/ato/demergers.md"));
    assert!(
        include_str!("../docs/ato/demergers.md")
            .contains("you calculate the 12 months from the date of demerger if you did not"),
        "the cited ATO mirror still carries the no-rollover discount-clock rule"
    );
}

/// Docs-sync pin for the closed-year decision (SCENARIOS A-15/A-21/A-25/A-35,
/// 2026-08-15): the restatement exposure is real but not a bug — there is no
/// lodged-year concept in the data model, and building one (a per-FY lodgement
/// marker plus a changed-since-lodgement flag over `row_history`) is a feature
/// deliberately not taken on. That decision is only honest if it is *stated*,
/// since a user reasonably assumes a prior year's numbers are settled — so
/// this is a documentation-only requirement, pinned here: the Known
/// limitations entry, the two properties that make it survivable (auditable
/// via row history, snapshots don't cover tax reports), the mitigation, the
/// A-40 exchange-holiday footnote, and the README's own scope-cuts summary.
#[test]
fn closed_year_restatement_documented() {
    let limitations = known_limitations();
    assert!(limitations.contains(
        "**A lodged financial year can be restated with nothing marking it** (2026-08-15)"
    ));
    assert!(limitations.contains("**no financial year is ever closed**"));
    // The two facts that bound the exposure: it is recoverable after the fact,
    // and the one stored-report mechanism that sounds like it would help
    // deliberately does not.
    assert!(limitations.contains("auditable after the fact"));
    assert!(limitations.contains(
        "[Report snapshots](#report-snapshots) do not help here either; they persist the three \
         price-dependent reports only, never a tax report"
    ));
    // The mitigation the user is left with, and the low-severity companion.
    assert!(limitations.contains("save the [annual tax report](#annual-tax-report) as a PDF"));
    assert!(limitations.contains("[`DELETE /exchange_holidays/:mic/:date`](#exchange-holidays)"));
    assert!(limitations.contains("a record field, not a tax figure"));
    assert!(README_MD.contains("**no financial year is ever closed**"));
}

/// Docs-sync pin for the as-at-date convention shared by every holdings
/// report (2026-08-16, SCENARIOS E-14): an undated report is the position as
/// at *today*, so a corporate action or trade recorded ahead of its effective
/// date — the normal way terms are recorded — is not in force yet, and the
/// undated reports can't disagree with the dated ones run for today. The
/// behaviour itself is pinned by `domain::open_parcels::tests`.
#[test]
fn as_at_today_convention_documented() {
    assert!(API_MD.contains("### As-at date"));
    assert!(
        API_MD.contains(
            "a report that takes no date is as at **today** — never \"every fact on file\""
        )
    );
    assert!(API_MD.contains(
        "A [trade](#trades) or [corporate action](#corporate-actions) dated in the future is \
         recorded but not yet in force"
    ));
    // The two halves that make it one rule: trades are bounded too, and the
    // FY-keyed reports deliberately are not.
    assert!(
        API_MD.contains("A future-dated *trade* is bounded the same way rather than carved out")
    );
    assert!(
        API_MD.contains("The [realised](#realised-gains) and FY-keyed tax reports are not bounded")
    );
    // The undated reports name themselves as as-at-today where they are
    // documented, not only in the shared section.
    assert!(API_MD.contains("as at today (see [As-at date](#as-at-date))"));
}

/// Docs-sync pin for the AMIT/E4 mutual exclusion (2026-08-16, SCENARIOS
/// E-04): a return of capital is refused on a listing flagged `amit`, whose
/// cost-base movement is its AMMA statement's `cost_base_adjustment`. Both
/// ends say so — the corporate-actions write rules and the AMIT-adjustments
/// section — and the Response-codes `422` row lists the refusal. The
/// behaviour itself is pinned by `entities::corporate_action::tests`.
#[test]
fn amit_return_of_capital_refusal_documented() {
    assert!(API_MD.contains(
        "A `ReturnOfCapital` on a listing flagged [`amit`](#listings) is rejected with `422` \
         outright"
    ));
    assert!(API_MD.contains("the two paths are **mutually exclusive**"));
    // The AMIT side names both doors, so a reader arriving from the AMMA
    // statement sees why neither other path is open to it.
    assert!(API_MD.contains("**This is an AMIT's only cost-base movement.**"));
    assert!(API_MD.contains(
        "a `tax_deferred_amount` on its [income](#income) rows and a `ReturnOfCapital` on the \
         listing are each refused `422`"
    ));
    // The converted-fund case the cost-base chain still has to handle: the
    // refusal is on the write, so pre-conversion payments stand.
    assert!(API_MD.contains("record the pre-conversion payments before flagging the listing"));
    assert!(
        API_MD.contains(
            "since a `ReturnOfCapital` on an already-flagged AMIT is refused at write time"
        )
    );
    assert!(API_MD.contains(
        "a `ReturnOfCapital` on a listing flagged `amit` (an AMIT's cost-base movement is its \
         AMMA statement's `cost_base_adjustment`, CGT event E10, not E4)"
    ));
}

/// Docs-sync pin for the write-time state check that bounds that open edit
/// path (2026-08-15): a corporate-action write is refused when the resulting
/// terms would leave a sale allocating more units than its parcel holds, so
/// the Corporate actions section states what is re-checked (and that both
/// listings are, on a move), and the Response-codes `422` row lists the
/// refusal. The behaviour itself is pinned by
/// `entities::corporate_action::tests`.
#[test]
fn corporate_action_write_state_check_documented() {
    assert!(API_MD.contains("**Writing terms that would over-consume a parcel.**"));
    assert!(API_MD.contains(
        "every write of a corporate action re-checks — in the write's own transaction — that \
         each parcel of the affected listing still covers the allocations drawn on it"
    ));
    assert!(API_MD.contains("**both** listings are re-checked"));
    assert!(API_MD.contains(
        "writing a corporate action whose terms would leave a sale allocating more units than \
         the parcel it draws on holds"
    ));
}

/// Docs-sync pin for serving the app under a reverse-proxy sub-path: the
/// deployment story lives in the README (including the nginx block, whose
/// two easy-to-get-wrong details — no trailing slash on `proxy_pass`, and
/// an upload limit above the 25 MB attachment cap — are the whole reason
/// the snippet is shipped rather than left to the reader), and the API doc
/// states that every documented path moves under the prefix.
#[test]
fn reverse_proxy_base_path_documented() {
    assert!(README_MD.contains("### Behind a reverse proxy"));
    assert!(README_MD.contains("`--base-path`"));
    assert!(README_MD.contains("location /share_tracker/ {"));
    assert!(README_MD.contains("proxy_pass http://127.0.0.1:3000;"));
    assert!(README_MD.contains("No trailing slash on proxy_pass"));
    assert!(README_MD.contains("client_max_body_size 25m;"));
    assert!(README_MD.contains("X-Forwarded-Proto"));
    // API.md: the prefix applies to every documented path, and the
    // trailing-slash redirect is a documented response code.
    assert!(API_MD.contains("**Base path.**"));
    assert!(API_MD.contains("`GET /listings` becomes `GET /share_tracker/listings`"));
    assert!(API_MD.contains("`307 Temporary Redirect`"));
}

/// Docs-sync pin for the optional `[auth]` shared-credential access control
/// (`infra::auth`): the README surfaces it as a Feature and documents how to
/// configure and generate credentials for it; the API doc has its own
/// section plus the `401`/`303` response codes and the two accepted
/// limitations (cookie revocation, login CSRF) it introduces.
#[test]
fn authentication_documented() {
    // README: the Features bullet, the rewritten no-auth notes, the
    // dedicated section and its config/CLI-helper examples.
    assert!(README_MD.contains("**Authentication (optional)**"));
    assert!(README_MD.contains("### Authentication"));
    assert!(README_MD.contains("unless [`[auth]`](#authentication) is configured"));
    assert!(README_MD.contains("share-tracker hash-password"));
    assert!(README_MD.contains("share-tracker gen-token"));
    assert!(README_MD.contains("[auth]"));
    assert!(README_MD.contains("password_hash"));
    // API.md: the preamble line, the dedicated section, the login/logout
    // endpoints, and the response-code/known-limitations entries.
    assert!(API_MD.contains("**Authentication.**"));
    assert!(API_MD.contains("## Authentication"));
    assert!(API_MD.contains("GET` | `/login`"));
    assert!(API_MD.contains("POST` | `/login`"));
    assert!(API_MD.contains("POST` | `/logout`"));
    assert!(API_MD.contains("`303 See Other`"));
    assert!(API_MD.contains("`401 Unauthorized`"));
    assert!(known_limitations().contains("session cookies aren't revocable"));
    assert!(known_limitations().contains("no CSRF token"));
}

/// Docs-sync pin for deletes blocked by an inbound foreign key (SCENARIOS
/// A-18, A-23, A-38, A-41). The behaviour itself is tested in
/// `entities::tests`; what needs pinning here is the documentation half —
/// the shared explainer, the two directions the same `422` can mean, and the
/// A-23 dead end (a listing that has ever carried a **manual** closing price
/// can never be deleted), which is a consequence of two documented rules and
/// was stated by neither.
#[test]
fn deletes_blocked_by_a_dependant_documented() {
    // The shared section, its wording, and both directions of the 422.
    assert!(API_MD.contains("## Deletes blocked by a dependant"));
    assert!(API_MD.contains("this listing is still referenced by closing prices (2)"));
    assert!(API_MD.contains("the request refers to a record that does not exist"));
    assert!(API_MD.contains("There is no cascade delete"));
    // The response-code table and the error-bodies note point at it.
    assert!(API_MD.contains(
        "any `DELETE` of a row another table still references — see \
         [Deletes blocked by a dependant](#deletes-blocked-by-a-dependant)"
    ));
    assert!(API_MD.contains("except a blocked delete, whose body names the dependants instead"));
    // The A-23 dead end, stated where a reader would look for it.
    assert!(API_MD.contains(
        "**A listing that has ever had a closing price [entered by hand](#closing-prices) can no \
         longer be deleted at all.**"
    ));
    // The two entity sections whose blocked delete needed explaining.
    assert!(API_MD.contains("There is no cascade: each adjustment is removed individually"));
    assert!(API_MD.contains("`DELETE` returns `422` while anything still references the exchange"));
}

/// Docs-sync pin for fractional entitlements (SCENARIOS E-11, E-36). The
/// behaviour — every ratio-driven action keeping the exact fraction its ratio
/// produces — is pinned by tests in the corporate-action and rollover modules;
/// what needed writing down is the *convention* (registry rounding and
/// cash-in-lieu are deliberately not modelled), stated for all four ratio
/// actions rather than only `ShareSplit` and `Demerger`, and the answer to the
/// question it leaves the reader with: what to do with the cash actually
/// received for a fraction.
#[test]
fn fractional_entitlements_documented() {
    // The shared section: the convention, its reason, and both registry
    // practices it declines to model.
    assert!(API_MD.contains("### Fractional entitlements"));
    assert!(API_MD.contains(
        "Every ratio-driven action — `ShareSplit`, `BonusIssue`, `ScripForScrip`, `Demerger` — \
         keeps the **exact fractional quantity** its ratio produces"
    ));
    assert!(API_MD.contains("sell the aggregated fractions on-market and pay **cash in lieu**"));
    assert!(API_MD.contains("would silently lose (or invent) part of a parcel"));
    // What to do with the cash: a CGT event on the disposed fraction, entered
    // as an ordinary Sell — not a rounding to be absorbed.
    assert!(API_MD.contains(
        "it is the disposal of that fraction and its own (small) CGT event, not a bookkeeping \
         rounding: enter it as an ordinary [Sell](#sells) of the fractional units"
    ));
    assert!(API_MD.contains("A registry that rounds the entitlement *up* to a whole unit instead"));
    // E-11 / E-36: the two bullets that stated it for neither, with the
    // worked figures a reader can check their own entry against.
    assert!(API_MD.contains("a 1-for-10 issue on 105 units gives 10.5 bonus units"));
    assert!(API_MD.contains(
        "a 1-for-3 exchange of 101 units gives 33.666666666666666666666666667 replacement units"
    ));
    // …and the two that did, now pointing at the shared section.
    assert_eq!(
        API_MD
            .matches("[Fractional entitlements](#fractional-entitlements)")
            .count(),
        4
    );
}

/// Pins for the FreeBSD packaging + versioned-release pipeline (REQUIREMENTS
/// 2026-07-13). The release workflow and package skeleton are plain text CI
/// consumes, so these tests keep their load-bearing pieces from silently
/// drifting apart: the version flows Cargo.toml → build-pkg.sh → manifest,
/// every file the plist packages is staged, and the rc script drives the
/// config file the package installs.
mod freebsd_packaging {
    use super::README_MD;

    const RELEASE_YML: &str = include_str!("../.github/workflows/release.yml");
    const BUILD_SH: &str = include_str!("../pkg/freebsd/build-pkg.sh");
    const SMOKE_SH: &str = include_str!("../pkg/freebsd/smoke-test.sh");
    const MANIFEST: &str = include_str!("../pkg/freebsd/manifest.ucl");
    const PLIST: &str = include_str!("../pkg/freebsd/plist");
    const RC_SCRIPT: &str = include_str!("../pkg/freebsd/share_tracker");

    /// The version number has one source of truth: Cargo.toml. The build
    /// script reads it from there and substitutes the manifest's placeholder;
    /// the binary's --version comes from the same place (pinned in
    /// `infra::args`); the workflow reads it the same way for the tag.
    #[test]
    fn version_flows_from_cargo_toml_alone() {
        let extract = r#"sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml"#;
        assert!(BUILD_SH.contains(extract));
        assert!(SMOKE_SH.contains(extract));
        assert!(RELEASE_YML.contains(extract));
        assert!(MANIFEST.contains(r#"version: "__VERSION__""#));
        assert!(BUILD_SH.contains("sed \"s/__VERSION__/$VERSION/\""));
    }

    /// The workflow releases on push to main only when the version has no
    /// release yet, builds in a FreeBSD 15.1 VM (pkg ABI matches the
    /// deployment host), smoke-tests the installed package, and tags the
    /// exact commit that produced the pkg.
    #[test]
    fn release_workflow_shape() {
        assert!(RELEASE_YML.contains("branches: [main]"));
        assert!(RELEASE_YML.contains(r#"gh release view "v$version""#));
        assert!(RELEASE_YML.contains("vmactions/freebsd-vm"));
        assert!(RELEASE_YML.contains(r#"release: "15.1""#));
        // curl is the smoke test's POST client — fetch(1) can't POST and a
        // raw nc request half-closes early (hyper cancels the request).
        assert!(RELEASE_YML.contains("pkg install -y rust protobuf curl"));
        assert!(RELEASE_YML.contains("sh pkg/freebsd/build-pkg.sh"));
        assert!(RELEASE_YML.contains("sh pkg/freebsd/smoke-test.sh"));
        // The release tag points at the built commit, not a branch head.
        assert!(RELEASE_YML.contains(r#"--target "$GITHUB_SHA""#));
        assert!(RELEASE_YML.contains("share-tracker-*.pkg"));
        // Release notes come from the commits between tags, not the PR-based
        // --generate-notes (empty on a direct-to-main repo). The notes script
        // needs the previous tag, so the checkout must be full-history.
        assert!(RELEASE_YML.contains(r#"sh scripts/release-notes.sh "$VERSION""#));
        assert!(RELEASE_YML.contains("--notes-file notes.md"));
        assert!(RELEASE_YML.contains("fetch-depth: 0"));
        assert!(!RELEASE_YML.contains("--generate-notes"));
    }

    /// Executable check of `scripts/release-notes.sh` in a scratch git repo:
    /// the notes list exactly the commits after the previous tag (newest
    /// first, abbreviated SHA), link the tag-to-tag compare, exclude the
    /// release's own tag when re-run, and fall back to "Initial release."
    /// with the full history when no tag exists yet.
    #[test]
    fn release_notes_script_lists_commits_between_tags() {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/release-notes.sh");
        let dir = tempfile::tempdir().expect("tempdir");
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        let notes = |version: &str| {
            let out = std::process::Command::new("sh")
                .args([script, version])
                .current_dir(dir.path())
                .output()
                .expect("script runs");
            assert!(out.status.success(), "release-notes.sh: {out:?}");
            String::from_utf8(out.stdout).expect("utf-8 notes")
        };

        git(&["init", "-q"]);
        git(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Ancient pre-tag work",
        ]);

        // No tag yet: first-release mode lists the whole history.
        let first = notes("0.1.0");
        assert!(first.contains("Initial release."));
        assert!(first.contains("- Ancient pre-tag work ("));

        git(&["tag", "v0.1.0"]);
        git(&["commit", "-q", "--allow-empty", "-m", "Add feature X"]);
        git(&["commit", "-q", "--allow-empty", "-m", "Fix feature X"]);

        let body = notes("0.2.0");
        assert!(body.contains("## Changes since v0.1.0"));
        assert!(body.contains("- Add feature X ("));
        assert!(body.contains("- Fix feature X ("));
        // Only the commits after the previous tag appear.
        assert!(!body.contains("Ancient pre-tag work"));
        // Compare link spans previous tag to this release's tag.
        assert!(body.contains("/compare/v0.1.0...v0.2.0"));

        // Re-run after this release's tag exists (e.g. a retried job): the
        // notes still diff against the previous tag, not the release itself.
        git(&["tag", "v0.2.0"]);
        assert_eq!(notes("0.2.0"), body);
    }

    /// Every path the plist packages is staged by the build script, and vice
    /// versa nothing is staged that the plist would silently drop.
    #[test]
    fn plist_matches_staged_files() {
        for (plist_entry, staged) in [
            ("bin/share-tracker", "$STAGE/usr/local/bin/"),
            ("etc/rc.d/share_tracker", "$STAGE/usr/local/etc/rc.d/"),
            ("etc/share-tracker.toml.sample", "share-tracker.toml.sample"),
            ("etc/share-tracker.cron.sample", "share-tracker.cron.sample"),
            (
                "etc/newsyslog.conf.d/share-tracker.conf.sample",
                "newsyslog.conf.d/share-tracker.conf.sample",
            ),
        ] {
            assert!(PLIST.contains(plist_entry), "plist lists {plist_entry}");
            assert!(BUILD_SH.contains(staged), "build-pkg.sh stages {staged}");
        }
        // Four install lines in the script, four plist entries — a new staged
        // file must appear in both.
        let installs = BUILD_SH
            .lines()
            .filter(|l| l.starts_with("install "))
            .count();
        let entries = PLIST.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(installs, entries);
    }

    /// The plist must stay free of ports keywords: `pkg create` resolves
    /// `@sample` & co. from the ports tree (`/usr/ports/Keywords/*.ucl`),
    /// which the release VM doesn't have — an unknown keyword made pkg create
    /// emit nothing while still "succeeding" (the first v0.2.0 run). The
    /// sample→live copy semantics live in the manifest's own scripts instead,
    /// and both scripts hard-fail on any missed step.
    #[test]
    fn sample_configs_activate_without_ports_keywords() {
        assert!(!PLIST.contains('@'), "plist uses no ports keywords");
        // Manifest scripts re-implement @sample: copy into place on first
        // install, remove on deinstall only while unmodified.
        assert!(MANIFEST.contains("post-install"));
        assert!(MANIFEST.contains(r#"cp -p "/usr/local/etc/$f.sample" "/usr/local/etc/$f""#));
        assert!(MANIFEST.contains("pre-deinstall"));
        assert!(MANIFEST.contains(r#"cmp -s "/usr/local/etc/$f.sample" "/usr/local/etc/$f""#));
        // Every shipped config goes through that activation loop.
        assert_eq!(
            MANIFEST
                .matches("for f in share-tracker.toml share-tracker.cron newsyslog.conf.d/share-tracker.conf")
                .count(),
            2,
            "install and deinstall loops both cover every sample config"
        );
        // The scripts enforce -eu in the body (`sh script.sh` drops shebang
        // flags) and build-pkg.sh refuses to "succeed" without the artifact.
        assert!(BUILD_SH.contains("\nset -eu\n"));
        assert!(SMOKE_SH.contains("\nset -eu\n"));
        assert!(BUILD_SH.contains(r#"[ -f "$OUT/share-tracker-$VERSION.pkg" ]"#));
        // The smoke test proves activation happened: without these, a failed
        // copy goes unnoticed because the server answers HTTP on defaults.
        assert!(SMOKE_SH.contains("[ -f /usr/local/etc/share-tracker.toml ]"));
        assert!(SMOKE_SH.contains("[ -f /usr/local/etc/share-tracker.cron ]"));
        assert!(SMOKE_SH.contains("[ -f /usr/local/etc/newsyslog.conf.d/share-tracker.conf ]"));
        assert!(SMOKE_SH.contains("pw usershow share_tracker"));
    }

    /// The rc script points the server at the config file the package
    /// installs (the activated copy of `share-tracker.toml.sample`, whose
    /// parseability is pinned in `infra::config`), and the smoke test proves
    /// the service pieces before anything is released.
    #[test]
    fn rc_script_and_smoke_test_drive_the_installed_config() {
        assert!(RC_SCRIPT.contains("rcvar=share_tracker_enable"));
        assert!(
            RC_SCRIPT.contains(": ${share_tracker_config:=\"/usr/local/etc/share-tracker.toml\"}")
        );
        assert!(RC_SCRIPT.contains("--config ${share_tracker_config}"));
        // daemon(8) supervision: restart on exit, log to the server's own
        // file, and reopen it (-H, instead of forwarding SIGHUP) when the
        // shipped newsyslog config rotates it — rotation must never restart
        // the server.
        assert!(RC_SCRIPT.contains("command=\"/usr/sbin/daemon\""));
        assert!(RC_SCRIPT.contains(": ${share_tracker_logfile:=\"/var/log/share-tracker.log\"}"));
        assert!(RC_SCRIPT.contains("-H -o ${share_tracker_logfile}"));
        const NEWSYSLOG: &str = include_str!("../pkg/freebsd/newsyslog.conf");
        assert!(NEWSYSLOG.contains("/var/log/share-tracker.log"));
        assert!(NEWSYSLOG.contains("/var/run/share_tracker/share_tracker.pid"));
        // share_tracker_user is rc.subr's ${name}_user convention: the whole
        // daemon(8) chain runs su(1)'d to the service user, so the pidfile
        // lives in a service-user-owned subdirectory the precmd (re)creates
        // each start — /var/run itself is root-only and cleared at boot. A
        // bare /var/run/<name>.pid fails "Permission denied" (shipped broken
        // in v0.4.0).
        assert!(RC_SCRIPT.contains("pidfile=\"/var/run/${name}/${name}.pid\""));
        assert!(RC_SCRIPT.contains("start_precmd=\"share_tracker_precmd\""));
        assert!(RC_SCRIPT.contains("install -d -o \"${share_tracker_user}\" \"/var/run/${name}\""));
        // The pidfile holds the daemon(8) supervisor's pid, so procname must
        // stay at its rc.subr default ($command = daemon): overriding it to
        // the server binary made check_pidfile reject the pid and status/stop
        // report "not running" while the service kept running.
        assert!(!RC_SCRIPT.contains("procname="));
        // daemon(8)'s argv carries the server binary explicitly (a ${procname}
        // spelling once expanded empty, so daemon parsed --config as its own
        // option) and must NOT carry -u: rc.subr's ${name}_user su already
        // dropped privileges, and an unprivileged daemon -u fails
        // setusercontext (initgroups EPERM), which -r turns into a respawn
        // loop that never starts the server (the v0.4.1 release-blocker).
        assert!(
            RC_SCRIPT.contains("-r /usr/local/bin/share-tracker --config ${share_tracker_config}")
        );
        assert!(!RC_SCRIPT.contains("-u ${share_tracker_user}"));
        // Install creates the service user the rc script runs as.
        assert!(MANIFEST.contains("pw useradd share_tracker"));
        assert!(RC_SCRIPT.contains(": ${share_tracker_user:=\"share_tracker\"}"));
        // Smoke test: installed version agrees, rc script loads, HTTP answers.
        assert!(SMOKE_SH.contains("grep -qx \"share-tracker $VERSION\""));
        assert!(SMOKE_SH.contains("/usr/local/etc/rc.d/share_tracker rcvar"));
        assert!(SMOKE_SH.contains("http://127.0.0.1:3999/reports/health"));
        // …and the service really starts through the rc script (a direct
        // binary run can't catch rc plumbing like the pidfile bug above).
        assert!(SMOKE_SH.contains("service share_tracker onestart"));
        assert!(SMOKE_SH.contains("http://127.0.0.1:3000/reports/health"));
        assert!(SMOKE_SH.contains("[ -s /var/run/share_tracker/share_tracker.pid ]"));
        // Stop must be bounded (an unbounded onestop once wedged the release
        // VM for 20 minutes), must genuinely succeed — no `|| true` — and
        // must take the server down with the supervisor.
        assert!(SMOKE_SH.contains("\ntimeout 30 service share_tracker onestop\n"));
        assert!(SMOKE_SH.contains("server survived onestop"));
        // Rotation is proven end-to-end: force a newsyslog rotation, then
        // fresh log lines must land in the new file (daemon -H reopened it)
        // without a new startup banner (the server was not restarted).
        assert!(SMOKE_SH.contains("newsyslog -F /var/log/share-tracker.log"));
        assert!(SMOKE_SH.contains("daemon -H reopen failed"));
        assert!(SMOKE_SH.contains("server restarted on rotation"));
        // The file gets plain text: logging::init switches ANSI off when
        // stdout is not a terminal, and the smoke test holds that end-to-end.
        assert!(SMOKE_SH.contains("ANSI escape codes in log file"));
        // On failure the smoke test surfaces the server's own output (it
        // goes to syslog under daemon -S, invisible in the CI log) by
        // re-running the service invocation with output to a file.
        assert!(SMOKE_SH.contains("-o /tmp/svc-diag.log"));
    }

    /// README documents installing the package, the configuration file, and
    /// how a release is cut.
    #[test]
    fn readme_documents_packaging_and_versioning() {
        assert!(README_MD.contains("## Installing on FreeBSD"));
        assert!(README_MD.contains("sysrc share_tracker_enable=YES"));
        assert!(README_MD.contains("`/var/log/share-tracker.log`"));
        assert!(README_MD.contains("newsyslog"));
        assert!(README_MD.contains("### Configuration file"));
        assert!(README_MD.contains("**CLI flag > config-file value > built-in default**"));
        assert!(README_MD.contains("## Releases and versioning"));
        assert!(README_MD.contains("**Cutting a release = bumping `version` in `Cargo.toml`**"));
    }
}
