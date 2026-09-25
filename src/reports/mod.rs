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

    /// Every route registered under `src/reports/*.rs` as a `(path, verb)`
    /// pair, sorted and de-duplicated. The verb is whichever of
    /// `get(...)`/`post(...)` the route registration itself names, so the
    /// table is read out of the route registration rather than kept by hand.
    /// The path literal may sit on the route call's own line or the next one,
    /// so this reads the first string literal after each route call and then
    /// the first routing fn after it. The needle is assembled rather than
    /// written out so this module does not match itself (it walks `mod.rs`
    /// too).
    fn report_routes() -> Vec<(String, &'static str)> {
        let needle = format!(".{}(", "route");
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/reports");
        let mut routes = Vec::new();
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
                let route = after[..close].to_string();
                let tail = &after[close..];
                let verb = match (tail.find("get("), tail.find("post(")) {
                    (Some(g), Some(p)) => {
                        if g < p {
                            "GET"
                        } else {
                            "POST"
                        }
                    }
                    (Some(_), None) => "GET",
                    (None, Some(_)) => "POST",
                    (None, None) => panic!(
                        "report route `{route}` registers neither `get(...)` nor `post(...)`"
                    ),
                };
                routes.push((route, verb));
                rest = tail;
            }
        }
        routes.sort();
        routes.dedup();
        routes
    }

    /// Every route path string in `src/reports/*.rs`, sorted and de-duplicated.
    fn report_route_paths() -> Vec<String> {
        let mut paths: Vec<String> = report_routes().into_iter().map(|(path, _)| path).collect();
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

    /// The `/report_snapshots/*` routes that answer `POST`: the generation
    /// writes that persist snapshots. `/report_snapshots/*` is the resource
    /// surface over the `report_snapshots` table, not a report path (the
    /// namespace rule in `docs/API.md` and the case test above both say so),
    /// so the read-verb rule does not reach it — its `POST`s write. Named
    /// rather than skipped, so a new `POST` there is a deliberate
    /// classification and a report route on the wrong verb still fails.
    const SNAPSHOT_WRITES: [&str; 3] = [
        "/report_snapshots/generate",
        "/report_snapshots/regenerate_all",
        "/report_snapshots/regenerate_provisional",
    ];

    /// The report surface's verb table (REST API audit 2026-09-24): a report
    /// is a **read**, so it is a `GET` with its parameters in the query string
    /// unless what it takes genuinely cannot be expressed in one. Exactly four
    /// report routes keep a `POST` body, each because its body carries a
    /// **map or a list**, not a scalar:
    ///
    /// - `/portfolio/overview`, `/portfolio/performance` and
    ///   `/portfolio/unrealised-gains` take the price-override map
    ///   (`{"prices": {"<listing_id>": "<price>"}}`) that a what-if run
    ///   supplies, beside `live` and `as_of_date`;
    /// - `/portfolio/net-capital-gain/what-if` takes a contemplated-disposal
    ///   body whose `allocations` are a list of per-parcel inputs.
    ///
    /// Every other report route is a `GET` — including the scalar-parameter
    /// reads this audit moved off `POST` (`/portfolio/activity` with
    /// `listing_id`/`price`, `/portfolio/period-performance` with
    /// `from`/`to`, `/portfolio/parcel-optimiser` with `listing_id`/
    /// `holding_account_id`/`units`/`sale_date`/`price`,
    /// `/reports/wash_sales` with `window_days`, `/reports/row_history` with
    /// its browse parameters, `/reports/tax_report` with `tax_year`, and
    /// `/reports/franking_at_risk/what-if` with `listing_id`/`sale_date`/
    /// `units`) — and the newly added ones with it. The offending path is
    /// named on failure.
    #[test]
    fn report_routes_use_the_read_verb_their_parameters_call_for() {
        /// The four POST-bodied report reads, each with the reason its body
        /// is a map or a list a query string cannot carry.
        const POST_BODIES: [(&str, &str); 4] = [
            (
                "/portfolio/overview",
                "the price-override map (beside `live` and `as_of_date`)",
            ),
            (
                "/portfolio/performance",
                "the price-override map (beside `live` and `as_of_date`)",
            ),
            (
                "/portfolio/unrealised-gains",
                "the price-override map (beside `live` and `as_of_date`)",
            ),
            (
                "/portfolio/net-capital-gain/what-if",
                "a contemplated-disposal body whose allocations are a list",
            ),
        ];

        let routes = report_routes();
        assert!(!routes.is_empty(), "the walk found no report routes");

        for (path, verb) in &routes {
            let expected = if path.starts_with("/report_snapshots") {
                if SNAPSHOT_WRITES.contains(&path.as_str()) {
                    "POST"
                } else {
                    "GET"
                }
            } else if POST_BODIES.iter().any(|(p, _)| p == path) {
                "POST"
            } else {
                "GET"
            };
            assert_eq!(
                *verb, expected,
                "report route `{path}` is registered `{verb}` but must be `{expected}` — a \
                 scalar-parameter read is `get(...)` with a query string; only a body carrying a \
                 map or a list (or a snapshot-generation write) stays `post(...)`"
            );
        }

        // The POST-bodied paths the table names are real routes, so a rename
        // cannot leave an entry excusing nothing — and only the four listed
        // report reads keep a body.
        for (path, _) in POST_BODIES {
            assert!(
                routes.iter().any(|(p, _)| p == path),
                "the verb table names `{path}`, which is not a registered report route"
            );
        }
        let body_posts: Vec<&str> = routes
            .iter()
            .filter(|(path, verb)| *verb == "POST" && !path.starts_with("/report_snapshots"))
            .map(|(path, _)| path.as_str())
            .collect();
        let mut expected_body_posts: Vec<&str> =
            POST_BODIES.iter().map(|(path, _)| *path).collect();
        expected_body_posts.sort_unstable();
        assert_eq!(
            body_posts, expected_body_posts,
            "unexpected POST-bodied report reads: {body_posts:?}"
        );
    }
}
