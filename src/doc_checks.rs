//! Tests pinning documentation-only requirements (a TODO item is only done when
//! a test exists for it — CLAUDE.md). Each test asserts its required text —
//! typically a Known-limitations entry in `docs/API.md`, its README surfacing,
//! and the cited ATO mirror — is present, so the documented scope cut can't
//! silently vanish.

const API_MD: &str = include_str!("../docs/API.md");
const README_MD: &str = include_str!("../README.md");

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
