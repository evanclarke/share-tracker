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

    /// Every route registered under `src/reports/**/*.rs` as a `(path, verb)`
    /// pair, sorted and de-duplicated. The verb is whichever of
    /// `get(...)`/`post(...)` the route registration itself names, so the
    /// table is read out of the route registration rather than kept by hand.
    /// The path literal may sit on the route call's own line or the next one,
    /// so this reads the first string literal after each route call and then
    /// the first routing fn **inside that same call** (bounded at the next
    /// route registration, so a call carrying two verbs cannot be mis-read from
    /// a later route's verb). The needle is assembled rather than written out so
    /// this module does not match itself (it walks `mod.rs` too).
    ///
    /// The walk is **recursive**: an entity module split into submodules
    /// (`trade.rs` → `trade/…`) is the documented growth path, and a
    /// non-recursive scan would silently stop seeing a report that had been
    /// split into `reports/<name>/http.rs`.
    fn report_routes() -> Vec<(String, &'static str)> {
        let needle = format!(".{}(", "route");
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/reports");
        let mut sources = Vec::new();
        collect_report_sources(&dir, &mut sources);
        assert!(
            sources.len() >= 30,
            "the walk found only {} report sources — it has stopped reading the directory",
            sources.len()
        );
        let mut routes = Vec::new();
        for (file, body) in &sources {
            let mut rest = body.as_str();
            while let Some(at) = rest.find(&needle) {
                rest = &rest[at + needle.len()..];
                let Some(open) = rest.find('"') else { break };
                let after = &rest[open + 1..];
                let Some(close) = after.find('"') else { break };
                let route = after[..close].to_string();
                let tail = &after[close..];
                let call = &tail[..tail.find(&needle).unwrap_or(tail.len())];
                let verb = match (call.find("get("), call.find("post(")) {
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
                        "report route `{route}` in {file} registers neither `get(...)` nor \
                         `post(...)` — the reports surface should have no other verb"
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

    /// Every `.rs` file under `dir`, recursively, as `(path, contents)`.
    fn collect_report_sources(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir)
            .expect("src/reports should be readable")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                collect_report_sources(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                out.push((
                    path.display().to_string(),
                    std::fs::read_to_string(&path).expect("report source should be readable"),
                ));
            }
        }
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
    /// table (its own `## Report snapshots` docs section), not a report path —
    /// but it is **uniformly snake_case** too. Its one kebab segment
    /// (`holding-series`) was renamed `holding_series` rather than excused, so
    /// the surface has a single case rule like the other two, and this test
    /// reaches it instead of skipping it. A report route invented in a fourth
    /// namespace still fails.
    #[test]
    fn report_paths_use_their_namespace_case() {
        let paths = report_route_paths();
        assert!(!paths.is_empty(), "the walk found no report routes");

        for path in &paths {
            let (namespace, separator, case, rest) =
                if let Some(rest) = path.strip_prefix("/portfolio/") {
                    ("/portfolio/", '-', "kebab-case", rest)
                } else if let Some(rest) = path.strip_prefix("/reports/") {
                    ("/reports/", '_', "snake_case", rest)
                } else if let Some(rest) = path.strip_prefix("/report_snapshots") {
                    ("/report_snapshots", '_', "snake_case", rest)
                } else {
                    panic!(
                        "report route `{path}` is outside `/portfolio/`, `/reports/` and \
                         `/report_snapshots/`"
                    );
                };
            let rest = rest.trim_start_matches('/');
            if rest.is_empty() {
                continue;
            }
            for (i, segment) in rest.split('/').enumerate() {
                // An axum path parameter (`{id}`) names no case — it is a value,
                // not a segment of the path's spelling — so the case rule does
                // not reach it and the message below is not about one.
                if segment.starts_with('{') && segment.ends_with('}') {
                    continue;
                }
                if i > 0 && SHARED_QUALIFIER_SEGMENTS.contains(&segment) {
                    continue;
                }
                assert!(
                    segment_matches(segment, separator),
                    "report route `{path}`: segment `{segment}` after {namespace} must be {case}"
                );
            }
        }

        // The endpoints renamed by the audit, named explicitly: the case check
        // above would already refuse the old spellings, and this pins that the
        // renames landed rather than the paths having vanished.
        for renamed in [
            "/reports/tax_report",
            "/reports/tax_report/years",
            "/report_snapshots/holding_series",
        ] {
            assert!(
                paths.iter().any(|p| p.as_str() == renamed),
                "the renamed route `{renamed}` is missing"
            );
        }
    }

    /// The UI config and the route table agree (REST API audit 2026-09-24):
    /// every `/reports/*` endpoint is driven from one of the served JS modules
    /// — the `REPORTS` config, the annual tax report's own renderer, or
    /// `app.js`, where the health banner calls `/reports/health` (deliberately
    /// not a `REPORTS` entry, it drives the cross-view banner) and the annual
    /// tax report's year picker calls `/reports/tax_report/years`. A
    /// `/reports/*` route with no UI caller at all fails here.
    ///
    /// The surface is the **served bundle** (`web::served_js_bundle`, the
    /// `JS_MODULES` concatenation), not three `include_str!`s by name, so a
    /// module split — the documented growth path for this UI — is covered
    /// automatically rather than leaving the caller in a fourth module the test
    /// cannot see.
    #[test]
    fn report_routes_have_a_ui_caller() {
        let ui = crate::web::served_js_bundle();
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

    /// A misspelt or unparseable query parameter on a report read is refused
    /// `400` naming it, never silently defaulted — the same boundary the entity
    /// lists pin (`entities::tests::every_list_route_refuses_an_unknown_parameter`),
    /// applied to the scalar report reads the audit moved onto `GET`. The
    /// highest-stakes cases are the optimiser and the activity ledger, where a
    /// silently-defaulted `price` is a wrong **money** figure.
    #[tokio::test]
    async fn every_query_decoding_report_read_refuses_an_unknown_parameter() {
        use crate::test_support::{ApiClient, test_pool};
        use axum::http::StatusCode;

        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        // (path, a parameter the route does take, so the refusal is about the
        // unknown one rather than a missing required one)
        for (path, known) in [
            ("/portfolio/activity", "listing_id=1"),
            ("/portfolio/parcel-optimiser", "listing_id=1"),
            ("/portfolio/period-performance", "from=2024-01-02"),
            ("/reports/row_history", "table=trades"),
            ("/reports/tax_report", "tax_year=2026"),
            ("/reports/wash_sales", "window_days=30"),
            ("/portfolio/open-parcels", "as_of_date=2024-01-02"),
        ] {
            let resp = client
                .get(&format!("{path}?{known}&zzz_unrecognised=1"))
                .await;
            assert_eq!(
                resp.status,
                StatusCode::BAD_REQUEST,
                "GET {path} must refuse an unknown parameter, not default it: {}",
                resp.text()
            );
            assert!(
                resp.text().contains("zzz_unrecognised"),
                "the {path} 400 must name the offending parameter: {}",
                resp.text()
            );
        }
        // …and an unparseable value for a parameter the route does take.
        let resp = client.get("/reports/tax_report?tax_year=not-a-year").await;
        assert_eq!(
            resp.status,
            StatusCode::BAD_REQUEST,
            "an unparseable tax_year must be refused: {}",
            resp.text()
        );
    }
}
