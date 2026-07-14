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

/// Docs-sync pin for the backup pipeline hardening (2026-07-13 improvement
/// review): the README documents verification (integrity check + migrations
/// match, `.bad` quarantine), the retention policy (newest 8 + 12 monthly
/// keepers, pattern-matched files only), and — the recorded decision for the
/// off-machine copy — that offsite sync is a documented external step, not an
/// in-scope job step; the jobs API documents the backup job's failure
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
    // The off-machine copy decision: documented external setup, not a job step.
    assert!(README_MD.contains("### Off-machine copies"));
    assert!(
        README_MD.contains("deliberately a documented external step, not a job inside the server")
    );
    assert!(README_MD.contains("rclone sync /mnt/backups"));
    // The jobs API documents the backup job's verify/prune/failure semantics.
    assert!(API_MD.contains("`POST /jobs/backup` returns `500`"));
    assert!(API_MD.contains("quarantined as `<name>.db.bad`"));
    assert!(API_MD.contains(
        "the newest 8 backups plus the first backup of each of the 12 most recent months"
    ));
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
        assert!(RELEASE_YML.contains("pkg install -y rust protobuf"));
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
        // Both shipped configs go through that activation loop.
        assert!(MANIFEST.contains("for f in share-tracker.toml share-tracker.cron"));
        // The scripts enforce -eu in the body (`sh script.sh` drops shebang
        // flags) and build-pkg.sh refuses to "succeed" without the artifact.
        assert!(BUILD_SH.contains("\nset -eu\n"));
        assert!(SMOKE_SH.contains("\nset -eu\n"));
        assert!(BUILD_SH.contains(r#"[ -f "$OUT/share-tracker-$VERSION.pkg" ]"#));
        // The smoke test proves activation happened: without these, a failed
        // copy goes unnoticed because the server answers HTTP on defaults.
        assert!(SMOKE_SH.contains("[ -f /usr/local/etc/share-tracker.toml ]"));
        assert!(SMOKE_SH.contains("[ -f /usr/local/etc/share-tracker.cron ]"));
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
        // daemon(8) supervision: restart on exit, logs to syslog.
        assert!(RC_SCRIPT.contains("command=\"/usr/sbin/daemon\""));
        // Install creates the service user the rc script runs as.
        assert!(MANIFEST.contains("pw useradd share_tracker"));
        assert!(RC_SCRIPT.contains(": ${share_tracker_user:=\"share_tracker\"}"));
        // Smoke test: installed version agrees, rc script loads, HTTP answers.
        assert!(SMOKE_SH.contains("grep -qx \"share-tracker $VERSION\""));
        assert!(SMOKE_SH.contains("/usr/local/etc/rc.d/share_tracker rcvar"));
        assert!(SMOKE_SH.contains("http://127.0.0.1:3999/reports/health"));
    }

    /// README documents installing the package, the configuration file, and
    /// how a release is cut.
    #[test]
    fn readme_documents_packaging_and_versioning() {
        assert!(README_MD.contains("## Installing on FreeBSD"));
        assert!(README_MD.contains("sysrc share_tracker_enable=YES"));
        assert!(README_MD.contains("### Configuration file"));
        assert!(README_MD.contains("**CLI flag > config-file value > built-in default**"));
        assert!(README_MD.contains("## Releases and versioning"));
        assert!(README_MD.contains("**Cutting a release = bumping `version` in `Cargo.toml`**"));
    }
}
