//! Read-only reports over the entity tables: AUD-denominated aggregations
//! (portfolio, realised/unrealised gains, tax summary) plus reference-data
//! validation (exchange MIC validation, settlement-holiday coverage). The one
//! exception to "no writes" is `snapshot`, which persists the price-dependent
//! reports' daily results to `report_snapshots`.
use axum::Router;
use sqlx::SqlitePool;

/// Taxpayer assumption stated on every tax-report row: the rates are hard-wired
/// for an Australian-resident *individual* — the 50% CGT discount and the 50%
/// LIC capital gain deduction. Other entity types (SMSF/complying super 33⅓%,
/// company 0%, trust/partnership flow-through) are deliberately not modelled
/// (scope decision, 2026-06-07). Kept comma-free so it stays a single CSV field.
pub const TAXPAYER_BASIS: &str = "individual resident: 50% CGT discount; 50% LIC deduction";

pub mod activity;
pub mod amit_adjustment_cross_check;
pub mod amit_cash_cross_check;
pub mod attachments;
pub mod e4_cross_check;
pub mod export;
pub mod franking;
pub mod franking_at_risk;
pub mod fx_coverage;
pub mod health;
pub mod indexation_cross_check;
pub mod mic_validation;
pub mod net_capital_gain;
pub mod open_parcels;
pub mod parcel_optimiser;
pub mod performance;
pub mod period_performance;
pub mod portfolio;
pub mod realised_gains;
pub mod rollover_consistency;
pub mod row_history;
pub mod settlement_coverage;
pub mod snapshot;
pub mod tax_report;
pub mod tax_summary;
pub mod unrealised_gains;
pub mod valuation;
pub mod wash_sales;
/// The weekly portfolio summary email's content. A read-only composition of
/// the reports above with no routes of its own — the `weekly-summary` job is
/// its only caller, so it is not merged into `router` below.
pub mod weekly_summary;

/// A listing's ticker for a rejection or detail message — the error-bodies
/// contract (API.md) names entities by ticker/name, never by raw foreign-key
/// id. Falls back to `listing <id>` when no such row exists (e.g. an unknown
/// id straight from the request).
pub(crate) async fn listing_label(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<String, sqlx::Error> {
    let ticker: Option<String> = sqlx::query_scalar("SELECT ticker FROM listings WHERE id = ?")
        .bind(listing_id)
        .fetch_optional(pool)
        .await?;
    Ok(ticker.unwrap_or_else(|| format!("listing {listing_id}")))
}

/// A holding account's quoted name for a message (`account 'Default'`),
/// falling back to `account <id>` when no such row exists.
pub(crate) async fn account_label(
    pool: &SqlitePool,
    account_id: i64,
) -> Result<String, sqlx::Error> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM holding_accounts WHERE id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    Ok(name.map_or_else(
        || format!("account {account_id}"),
        |n| format!("account '{n}'"),
    ))
}

/// Merge every report's routes into a single router.
pub fn router() -> Router<SqlitePool> {
    portfolio::router()
        .merge(activity::router())
        .merge(open_parcels::router())
        .merge(performance::router())
        .merge(period_performance::router())
        .merge(unrealised_gains::router())
        .merge(realised_gains::router())
        .merge(net_capital_gain::router())
        .merge(parcel_optimiser::router())
        .merge(tax_summary::router())
        .merge(mic_validation::router())
        .merge(settlement_coverage::router())
        .merge(e4_cross_check::router())
        .merge(indexation_cross_check::router())
        .merge(amit_adjustment_cross_check::router())
        .merge(rollover_consistency::router())
        .merge(amit_cash_cross_check::router())
        .merge(wash_sales::router())
        .merge(row_history::router())
        .merge(franking_at_risk::router())
        .merge(fx_coverage::router())
        .merge(health::router())
        .merge(snapshot::router())
        .merge(tax_report::router())
        .merge(attachments::router())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    /// Every route path string in `src/reports/*.rs`, sorted and de-duplicated.
    /// The path literal may sit on the route call's own line or the next one,
    /// so this reads the first string literal after each route call rather than
    /// grepping line by line. The needle is assembled rather than written out
    /// so this module does not match itself (it walks `mod.rs` too).
    fn report_route_paths() -> Vec<String> {
        let needle = format!(".{}(", "route");
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/reports");
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(&dir)
            .expect("src/reports should be readable")
            .flatten()
        {
            let path = entry.path();
            if path.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            let body = std::fs::read_to_string(&path).expect("report source should be readable");
            let mut rest = body.as_str();
            while let Some(at) = rest.find(&needle) {
                rest = &rest[at + needle.len()..];
                let Some(open) = rest.find('"') else { break };
                let after = &rest[open + 1..];
                let Some(close) = after.find('"') else { break };
                paths.push(after[..close].to_string());
                rest = &after[close..];
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }

    /// Whether every byte of one path segment is a lowercase letter, a digit or
    /// `separator` — the shape the namespace's case names.
    fn segment_matches(segment: &str, separator: char) -> bool {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == separator)
    }

    /// The one sub-resource qualifier both report namespaces share: it
    /// qualifies an endpoint (`/what-if`) rather than naming a report, so the
    /// namespace case rule — which governs how a path names its report — does
    /// not reach it. Every other segment after the namespace does.
    const SHARED_QUALIFIER_SEGMENTS: [&str; 1] = ["what-if"];

    /// The report surface's path namespace/case rule (REST API audit
    /// 2026-09-24), pinned against the route table rather than the prose: a
    /// report path lives under `/portfolio/*` in **kebab-case** or `/reports/*`
    /// in **snake_case**, the naming segment and every qualifier after it
    /// included, so a new route in either namespace that breaks its case fails
    /// here with the offending path named.
    ///
    /// `/report_snapshots/*` is the resource surface over the `report_snapshots`
    /// table (its own `## Report snapshots` docs section), not a report path, so
    /// the case rule does not govern it — but it is still required to sit in
    /// that one namespace, and a report route invented anywhere else fails.
    #[test]
    fn report_paths_use_their_namespace_case() {
        let paths = report_route_paths();
        assert!(!paths.is_empty(), "the walk found no report routes");

        for path in &paths {
            if path == "/report_snapshots" || path.starts_with("/report_snapshots/") {
                continue;
            }
            let (namespace, separator, case, rest) =
                if let Some(rest) = path.strip_prefix("/portfolio/") {
                    ("/portfolio/", '-', "kebab-case", rest)
                } else if let Some(rest) = path.strip_prefix("/reports/") {
                    ("/reports/", '_', "snake_case", rest)
                } else {
                    panic!(
                        "report route `{path}` is outside `/portfolio/`, `/reports/` and \
                         `/report_snapshots/`"
                    );
                };
            for (i, segment) in rest.split('/').enumerate() {
                if i > 0 && SHARED_QUALIFIER_SEGMENTS.contains(&segment) {
                    continue;
                }
                assert!(
                    segment_matches(segment, separator),
                    "report route `{path}`: segment `{segment}` after {namespace} must be {case}"
                );
            }
        }

        // The two endpoints renamed by the audit, named explicitly: the case
        // check above would already refuse the old kebab spelling, and this pins
        // that the rename landed rather than the paths having vanished.
        assert!(paths.iter().any(|p| p.as_str() == "/reports/tax_report"));
        assert!(
            paths
                .iter()
                .any(|p| p.as_str() == "/reports/tax_report/years")
        );
    }

    /// The UI config and the route table agree (REST API audit 2026-09-24):
    /// every `/reports/*` endpoint is driven from one of the three served
    /// modules that call report endpoints — the `REPORTS` config, the annual tax
    /// report's own renderer, or `app.js`, where the health banner calls
    /// `/reports/health` (deliberately not a `REPORTS` entry, it drives the
    /// cross-view banner) and the annual tax report's year picker calls
    /// `/reports/tax_report/years`. A `/reports/*` route with no UI caller at
    /// all fails here.
    #[test]
    fn report_routes_have_a_ui_caller() {
        const CONFIG_JS: &str = include_str!("../web/config.js");
        const TAXREPORT_JS: &str = include_str!("../web/taxreport.js");
        const APP_JS: &str = include_str!("../web/app.js");
        let ui = [CONFIG_JS, TAXREPORT_JS, APP_JS].join("\n");
        for path in report_route_paths() {
            if !path.starts_with("/reports/") {
                continue;
            }
            assert!(
                ui.contains(path.as_str()),
                "report route `{path}` is served but called from no UI module"
            );
        }
    }
}
