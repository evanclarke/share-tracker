//! Serves the embedded single-page web frontend.
//!
//! The frontend is plain HTML/CSS/JS with no build step — the assets are
//! compiled into the binary with `include_str!` (the same approach as
//! `schedule.cron`), so the server has no runtime filesystem dependency and the
//! routes are testable with `oneshot`. The JS ships as native ES modules
//! (`<script type="module">` in the shell): `app.js` is the entry point (the
//! generic rendering engine and router) importing `config.js` (the
//! ENTITIES/REPORTS/ACTIONS configuration), `forms.js` (field constructors and
//! form wiring), `util.js` (shared utilities), and `taxreport.js` (the Annual
//! Tax Report's bespoke, non-`filterableTable` print-document renderer) —
//! each import specifier must have a matching `/static/...` route here. The
//! app is a hash-routed SPA that
//! talks to the existing JSON API, so `/` is the only HTML entry point (deep
//! links use `#/...` fragments, which never reach the server) and no SPA
//! fallback route is needed.
//!
//! The pure JS helpers are unit-tested by `src/web/*.test.js`, executed with
//! `node --test 'src/web/*.test.js'` (Node 22+, no build step —
//! `src/web/package.json` marks the tree as ES modules so Node parses the
//! files exactly as the browser does). Test files are never servable: this
//! `JS_MODULES` allowlist is the only route table, and the
//! `js_test_files_are_not_served_and_every_module_is` test pins that no
//! `*.test.js` file is listed on it (and that every non-test module is).
use axum::{
    Router,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};
use sqlx::SqlitePool;

const INDEX_HTML: &str = include_str!("web/index.html");
const STYLE_CSS: &str = include_str!("web/style.css");

/// The ES modules making up the app, as (route, source) pairs: the `app.js`
/// entry point plus everything it (transitively) imports. A new module is
/// served by adding a pair here.
const JS_MODULES: [(&str, &str); 7] = [
    ("/static/app.js", include_str!("web/app.js")),
    ("/static/chart.js", include_str!("web/chart.js")),
    ("/static/config.js", include_str!("web/config.js")),
    ("/static/forms.js", include_str!("web/forms.js")),
    ("/static/nav.js", include_str!("web/nav.js")),
    ("/static/taxreport.js", include_str!("web/taxreport.js")),
    ("/static/util.js", include_str!("web/util.js")),
];

/// Routes serving the frontend shell and its static assets. Returns a
/// `Router<SqlitePool>` purely so it merges with the entity/report routers; the
/// handlers are stateless.
///
/// `base_path` is the reverse-proxy prefix the application is nested under
/// (`""` at the root — see `app::router`). The routes themselves are unaware of
/// it, because `nest` strips it before matching; it exists here only to be
/// baked into the shell, which is the one place the frontend learns where it is
/// mounted. `auth_enabled` is likewise baked into the shell (as a `<meta
/// name="auth">`) so `nav.js` knows whether to render "Log out" and `util.js`
/// knows a 401 means "go to `/login`" rather than some other failure — the
/// frontend has no other way to learn whether `[auth]` is configured. The
/// shell is templated once at startup, not per request.
pub fn router(base_path: &str, auth_enabled: bool) -> Router<SqlitePool> {
    let shell = index_html(base_path, auth_enabled);
    let mut router = Router::new()
        .route(
            "/",
            get(move || {
                let shell = shell.clone();
                async move { html_asset(shell) }
            }),
        )
        .route("/static/style.css", get(style_css));
    for (path, source) in JS_MODULES {
        router = router.route(path, get(move || async move { js_asset(source) }));
    }
    router
}

/// The SPA shell with its build-time placeholders filled in: the crate
/// version shown in the header; the base path, which the shell carries in
/// both directions — as the prefix on its own asset URLs, and as the
/// `<meta name="base-path">` the frontend's `apiUrl` (util.js) reads to prefix
/// every API call it makes; and whether `[auth]` is configured, as
/// `<meta name="auth">` (`"1"` or `""`).
fn index_html(base_path: &str, auth_enabled: bool) -> String {
    INDEX_HTML
        .replace("{{VERSION}}", env!("CARGO_PKG_VERSION"))
        .replace("{{BASE}}", base_path)
        .replace("{{AUTH}}", if auth_enabled { "1" } else { "" })
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

fn js_asset(body: &'static str) -> Response {
    asset("text/javascript; charset=utf-8", body)
}

fn html_asset(body: String) -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

async fn style_css() -> Response {
    asset("text/css; charset=utf-8", STYLE_CSS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, ApiResponse};
    use axum::http::StatusCode;

    async fn get(uri: &str) -> ApiResponse {
        get_based("", uri).await
    }

    /// The frontend routes as served under a given base path. The routes are
    /// nested by `app::router`, so this router still serves them unprefixed —
    /// only the shell's contents change.
    async fn get_based(base_path: &str, uri: &str) -> ApiResponse {
        ApiClient::over(
            router(base_path, false).with_state(SqlitePool::connect(":memory:").await.unwrap()),
        )
        .get(uri)
        .await
    }

    async fn body_string(resp: ApiResponse) -> String {
        resp.text().to_string()
    }

    #[tokio::test]
    async fn index_is_served_as_html() {
        let resp = get("/").await;
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(
            resp.headers.get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let body = body_string(resp).await;
        // The shell loads the SPA assets and provides the mount point + nav.
        // The JS is native ES modules, so the entry script must load as one.
        assert!(body.contains("<script type=\"module\" src=\"/static/app.js\">"));
        assert!(body.contains("/static/style.css"));
        assert!(body.contains("id=\"app\""));
        assert!(body.contains("id=\"nav\""));
    }

    // The version shown in the header is substituted from the crate version
    // at serve time (single source of truth: Cargo.toml), not hardcoded or
    // left as the raw template placeholder.
    #[tokio::test]
    async fn index_shows_the_crate_version() {
        let body = body_string(get("/").await).await;
        assert!(body.contains(&format!("id=\"version\">v{}", env!("CARGO_PKG_VERSION"))));
        assert!(!body.contains("{{VERSION}}"));
    }

    // Under a reverse-proxy base path the shell must ask for its own assets at
    // the prefixed paths — the browser resolves them against the origin, not
    // the mount point — and must publish the prefix for the frontend's `apiUrl`
    // to put in front of every API call it makes.
    #[tokio::test]
    async fn index_carries_the_base_path_on_its_assets_and_in_its_meta_tag() {
        let body = body_string(get_based("/share_tracker", "/").await).await;
        assert!(body.contains("<meta name=\"base-path\" content=\"/share_tracker\">"));
        assert!(body.contains("<script type=\"module\" src=\"/share_tracker/static/app.js\">"));
        assert!(body.contains("href=\"/share_tracker/static/style.css\""));
        assert!(!body.contains("{{BASE}}"));
        // Nothing may be left pointing at the unprefixed asset routes: those
        // are outside the mount and would 404 behind the proxy.
        assert!(!body.contains("\"/static/"));
    }

    // The default (no prefix) is byte-for-byte the pre-base-path shell: the
    // placeholder collapses to nothing rather than leaving a stray separator.
    #[tokio::test]
    async fn no_base_path_leaves_the_asset_urls_at_the_root() {
        let body = body_string(get("/").await).await;
        assert!(body.contains("<meta name=\"base-path\" content=\"\">"));
        assert!(body.contains("src=\"/static/app.js\""));
        assert!(body.contains("href=\"/static/style.css\""));
        assert!(!body.contains("{{BASE}}"));
    }

    // The default: `[auth]` absent, so the shell tells the frontend there is
    // nothing to log in or out of.
    #[tokio::test]
    async fn no_auth_leaves_the_auth_meta_empty() {
        let body = body_string(get("/").await).await;
        assert!(body.contains("<meta name=\"auth\" content=\"\">"));
        assert!(!body.contains("{{AUTH}}"));
    }

    // With `[auth]` configured, the shell flags it so nav.js renders "Log
    // out" and util.js treats a 401 as "go to /login" rather than a generic
    // failure (see infra::auth's module doc).
    #[tokio::test]
    async fn auth_enabled_sets_the_auth_meta() {
        let resp = ApiClient::over(
            router("", true).with_state(SqlitePool::connect(":memory:").await.unwrap()),
        )
        .get("/")
        .await;
        let body = body_string(resp).await;
        assert!(body.contains("<meta name=\"auth\" content=\"1\">"));
    }

    #[tokio::test]
    async fn es_modules_are_served_as_javascript() {
        for (path, source) in JS_MODULES {
            let resp = get(path).await;
            assert_eq!(resp.status, StatusCode::OK, "{path}");
            assert_eq!(
                resp.headers.get(header::CONTENT_TYPE).unwrap(),
                "text/javascript; charset=utf-8",
                "{path}"
            );
            assert_eq!(body_string(resp).await, source, "{path}");
        }
    }

    // Every `./x.js` import specifier in the served modules must resolve to a
    // served `/static/x.js` route — a module added to the graph but not to
    // JS_MODULES would 404 at runtime and break the whole app.
    #[tokio::test]
    async fn every_module_import_is_served() {
        for (path, source) in JS_MODULES {
            for line in source.lines() {
                let Some((_, spec)) = line.split_once(" from './") else {
                    continue;
                };
                let module = spec.trim_end_matches(';').trim_end_matches('\'');
                let route = format!("/static/{module}");
                assert!(
                    JS_MODULES.iter().any(|(p, _)| *p == route),
                    "{path} imports {module}, but {route} is not served"
                );
            }
        }
    }

    // The recorded decision on how JS test files stay out of the served
    // bundle: `JS_MODULES` is an explicit allowlist (nothing under `src/web/`
    // is served unless listed), unit tests live beside the modules as
    // `src/web/*.test.js` (run by `node --test`, never by the server), and
    // this test keeps the two sets partitioned — a test file on the route
    // table, or a new module missing from it, fails here.
    #[test]
    fn js_test_files_are_not_served_and_every_module_is() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web");
        for entry in std::fs::read_dir(dir).expect("src/web exists") {
            let name = entry.expect("readable dir entry").file_name();
            let name = name.to_str().expect("utf-8 filename");
            if !name.ends_with(".js") {
                continue;
            }
            let route = format!("/static/{name}");
            let served = JS_MODULES.iter().any(|(p, _)| *p == route);
            if name.ends_with(".test.js") {
                assert!(!served, "{name} is a test file and must not be served");
            } else {
                assert!(
                    served,
                    "{name} is not in JS_MODULES — add its (route, include_str!) pair"
                );
            }
        }
    }

    #[tokio::test]
    async fn style_css_is_served_as_css() {
        let resp = get("/static/style.css").await;
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(
            resp.headers.get(header::CONTENT_TYPE).unwrap(),
            "text/css; charset=utf-8"
        );
    }

    // Each UI item maps to a view registered in the served app bundle — the
    // concatenation of every served ES module. Without a browser harness these
    // assert the view (and the API endpoint it drives) is present in the
    // shipped JS — the honest, testable limit of an embedded SPA.
    async fn app_js_body() -> String {
        let mut bundle = String::new();
        for (path, _) in JS_MODULES {
            bundle.push_str(&body_string(get(path).await).await);
        }
        bundle
    }

    // Every server URL the frontend emits must go through `apiUrl`, which puts
    // the base path in front of it. A root-absolute literal instead — a bare
    // `fetch('/…')` or an `href: '/…'` — works at the root and silently 404s
    // behind a reverse-proxy prefix, on exactly the paths (uploads, downloads,
    // CSV exports) least likely to be hit while testing at the root. This scans
    // the whole served bundle so a new one is caught at the source.
    //
    // Hash routes (`href: '#/…'`) are exempt and don't match: the browser
    // resolves them against the current document, prefix included.
    #[tokio::test]
    async fn no_module_bypasses_apiurl_with_a_root_absolute_url() {
        let js = app_js_body().await;
        for bad in ["fetch('/", "fetch(\"/", "href: '/", "href: \"/"] {
            assert!(
                !js.contains(bad),
                "a served module contains {bad:?} — wrap the path in apiUrl(...) \
                 so it works under a reverse-proxy base path"
            );
        }
        // …and the one central client does prefix.
        assert!(js.contains("fetch(apiUrl(path)"));
    }

    #[tokio::test]
    async fn exchange_management_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/exchanges"));
        // The session close time (gates same-day closing-price collection) is
        // editable alongside the other exchange fields.
        assert!(js.contains("close_time"));
    }

    #[tokio::test]
    async fn closing_prices_ui_present() {
        let js = app_js_body().await;
        // The price-history screen lists stored prices (incl. errored rows)
        // through the shared filterableTable…
        assert!(js.contains("viewClosingPrices"));
        assert!(js.contains("/closing_prices"));
        // …with the per-row re-fetch action and the backfill form driving the
        // two on-demand endpoints.
        assert!(js.contains("/closing_prices/fetch"));
        assert!(js.contains("/closing_prices/backfill"));
        // An errored row no re-fetch can fix is discardable, so the health
        // banner stops reporting it — offered on non-ok rows and on the rows
        // a listing's `unpriced_before` marker supersedes, the one span in
        // which a stored ok price may be deleted.
        assert!(js.contains("Discard"));
        assert!(js.contains("row.status !== 'ok' || row._superseded"));
        assert!(js.contains("l.unpriced_before && p.price_date < l.unpriced_before"));
        assert!(js.contains("'/closing_prices/' + row._listing_id"));
        // The price-import job is described in the Jobs view.
        assert!(js.contains("price-import"));
        // A price restated out of the provider's post-split basis shows both
        // figures — the stored one and what the provider served — and the
        // one-off repair job is described in the Jobs view.
        assert!(js.contains("price_as_observed"));
        assert!(js.contains("As served by provider"));
        assert!(js.contains("price-rebase"));
        // Each fetched row names the provider symbol it was fetched under, so
        // a backfill run with a one-off `symbol` override is visible on the
        // screen rather than indistinguishable from an ordinary fetch (0038).
        assert!(js.contains("fetched_symbol"));
        assert!(js.contains("Fetched under symbol"));
    }

    /// A whole superseded span is cleared from this screen in one request:
    /// the form drives the bulk endpoint, is offered only for listings that
    /// carry the marker (without one the server refuses), and says what the
    /// operation does and does not destroy.
    #[tokio::test]
    async fn clear_superseded_prices_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("Clear superseded prices"));
        assert!(js.contains("/closing_prices/clear_unpriced_before"));
        // Only listings declaring the marker are offered.
        assert!(js.contains("return !!l.unpriced_before;"));
        // The screen states the two facts that make the clear safe.
        assert!(js.contains("excluded from valuation whatever is "));
        assert!(js.contains("stays in Row "));
    }

    /// A day the provider cannot serve is priced by hand from this screen:
    /// the form drives the manual-price PUT with both provenance fields, the
    /// columns show where every stored price came from, and a manual row
    /// offers no Re-fetch (the server refuses one).
    #[tokio::test]
    async fn manual_price_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("Manual price"));
        assert!(js.contains("'/closing_prices/' + Number(mListingSel.value)"));
        assert!(js.contains("sourced_from: mSourcedInp.value"));
        assert!(js.contains("reason: mReasonInp.value"));
        assert!(js.contains("'sourced_from', 'reason'"));
        assert!(js.contains("row.origin === 'manual'"));
        // The surrogate key is shown so a price can be looked up on the Row
        // History screen, which asks for the record's id.
        assert!(js.contains("id: p.id"));
        assert!(js.contains("const cols = ['id', 'listing', 'date'"));
    }

    #[tokio::test]
    async fn errored_prices_ui_present() {
        let js = app_js_body().await;
        // The Closing Prices screen surfaces health.errored_prices with a
        // Backfill action pre-filling the backfill form…
        assert!(js.contains("errored_prices"));
        assert!(js.contains("Listings with errored prices"));
        assert!(js.contains("/reports/health"));
        // …and the cross-view health banner links to it.
        assert!(js.contains("#/prices"));
        assert!(js.contains("Open Closing Prices"));
    }

    /// The missing-row counterpart: held days nothing ever fetched, beside
    /// the errored list and reusing the same Backfill action — over exactly
    /// the reported hole.
    #[tokio::test]
    async fn unpriced_days_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("unpriced_days"));
        assert!(js.contains("Listings with unpriced held days"));
        assert!(js.contains("fromInp.value = row.earliest_date"));
        assert!(js.contains("toInp.value = row.latest_date"));
    }

    /// The demerger's stated pre-demerger close: the four form fields in the
    /// Demerger group, and the health banner that names a demerger whose
    /// pre-demerger prices still carry the provider's spin-off adjustment.
    #[tokio::test]
    async fn demerger_stated_close_ui_present() {
        let js = app_js_body().await;
        for field in [
            "demerger_close_date",
            "demerger_close_price",
            "demerger_close_sourced_from",
            "demerger_close_reason",
        ] {
            assert!(
                js.contains(field),
                "{field} is not on the corporate-action form"
            );
        }
        assert!(js.contains("Demerger: last pre-demerger trading day"));
        assert!(js.contains("Demerger: actual close that day"));
        // The health banner drives the same endpoint and names the action.
        assert!(js.contains("demergers_missing_close"));
        assert!(js.contains("pre-demerger closing price(s) for "));
        assert!(js.contains("d.action_id"));
        // …and states the second figure too: the hand-entered prices in the
        // same span, which the stated close does not re-base.
        assert!(js.contains("d.manual_days"));
        assert!(js.contains("hand-entered price(s) ("));
    }

    /// The other demerger warning: the head listing and the entity the
    /// demerger created recorded the wrong way round, which the banner names
    /// by both tickers, both series and the date the head parcel is held
    /// from — linking to the screen the action is re-recorded on.
    #[tokio::test]
    async fn demerger_head_not_continuing_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("demergers_head_not_continuing"));
        assert!(js.contains("as its head listing, but "));
        assert!(js.contains("has no stored closing price before that date"));
        assert!(js.contains("d.head_unpriced_before"));
        assert!(js.contains("d.head_held_from"));
        assert!(js.contains("d.demerged_priced_days"));
        assert!(js.contains("d.demerger_ticker"));
        assert!(js.contains("the head and the new entity swapped"));
        assert!(js.contains("'#/e/corporate_actions'"));
    }

    #[tokio::test]
    async fn report_snapshots_ui_present() {
        let js = app_js_body().await;
        // The snapshot list + per-snapshot detail views drive the snapshot
        // endpoints (list, detail, on-demand generate/regenerate)…
        assert!(js.contains("viewSnapshots"));
        assert!(js.contains("viewSnapshotDetail"));
        assert!(js.contains("/report_snapshots"));
        assert!(js.contains("/report_snapshots/generate"));
        assert!(js.contains("#/r/snapshots"));
        // …with stale/provisional/carried-price snapshots badged and
        // regenerable per row.
        assert!(js.contains("m.stale ? 'stale'"));
        assert!(js.contains("(m.price_carried_forward ? 'carried' : 'ok')"));
        assert!(js.contains("Regenerate"));
        // The bulk repair buttons drive the two regeneration endpoints, and
        // the detail view explains a provisional snapshot.
        assert!(js.contains("/report_snapshots/regenerate_all"));
        assert!(js.contains("/report_snapshots/regenerate_provisional"));
        assert!(js.contains("Regenerate all"));
        assert!(js.contains("Regenerate provisional"));
        assert!(js.contains("snap.provisional"));
        // …and a carried-forward price, which nothing trues up — the detail
        // view names the way out (clear the listing's Unpriced from).
        assert!(js.contains("snap.price_carried_forward"));
        assert!(js.contains("Carried-forward price"));
        // …and a snapshot whose totals *omit* a holding (migration 0037):
        // its own badge, its own banner, and the list naming what left.
        assert!(js.contains("m.holding_excluded ? 'excluded'"));
        assert!(js.contains("snap.holding_excluded"));
        assert!(js.contains("Holding excluded"));
        assert!(js.contains("snap.excluded_holdings"));
        // Regenerate-all takes a date range, prefilled from the API's
        // default-range endpoint.
        assert!(js.contains("/report_snapshots/regenerate_range"));
        assert!(js.contains("rangeFromInp"));
        assert!(js.contains("rangeToInp"));
        // The time-series graph is built as inline SVG (no build step, no
        // chart library) from the series endpoint: market value and
        // unrealised gain over the stored snapshot dates.
        assert!(js.contains("/report_snapshots/series"));
        assert!(js.contains("seriesChart"));
        assert!(js.contains("createElementNS"));
        assert!(js.contains("polyline"));
        assert!(js.contains("market_value"));
        assert!(js.contains("unrealised_gain"));
        // The report-snapshot job is described in the Jobs view.
        assert!(js.contains("report-snapshot"));
        // The graph marks provisional points distinctly from stale ones, and
        // a point whose totals omit a holding distinctly from both — that is
        // where the line steps (migration 0037), so the tooltip names what
        // the total is missing.
        assert!(js.contains("p.provisional ? ' provisional' : ''"));
        assert!(js.contains("p.holding_excluded ? ' excluded' : ''"));
        assert!(js.contains("' (omits '"));
        assert!(js.contains("a total that omits a holding"));
        // The chart styles ship in the bundle too.
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains(".series-chart"));
        assert!(css.contains(".badge.stale"));
        assert!(css.contains(".badge.provisional"));
        assert!(css.contains(".badge.excluded"));
        assert!(css.contains("circle.provisional"));
        assert!(css.contains("circle.excluded"));
    }

    #[tokio::test]
    async fn cgt_decision_support_ui_present() {
        let js = app_js_body().await;
        // The parcel-optimiser screen drives the optimiser endpoint…
        assert!(js.contains("'parcel-optimiser'"));
        assert!(js.contains("/portfolio/parcel-optimiser"));
        // …and the pre-sale what-if screen the dry-run endpoint, with the
        // strategy picker naming each optimiser strategy.
        assert!(js.contains("'net-capital-gain-what-if'"));
        assert!(js.contains("/portfolio/net-capital-gain/what-if"));
        for strategy in ["'fifo'", "'min_gain'", "'max_discount'", "'harvest_losses'"] {
            assert!(js.contains(strategy), "missing strategy {strategy}");
        }
        // Both render through the generic report runner's params form and
        // per-key result tables (no bespoke views).
        assert!(js.contains("report.params"));
        assert!(js.contains("report.tables"));
    }

    /// SCENARIOS P-12: the taxpayer assumption behind the 50% discount is on
    /// screen wherever the discount is applied — as a table column on the
    /// reports whose rows each carry `taxpayer_basis` (realised gains, net
    /// capital gain, tax summary), and as a note under the header on the
    /// parcel optimiser, whose response states it once because the basis
    /// governs how the strategies are ranked against each other.
    #[tokio::test]
    async fn taxpayer_basis_is_shown_wherever_the_discount_applies() {
        let js = app_js_body().await;
        // The note is field-driven (any object response carrying the field),
        // not a bespoke view for one slug.
        assert!(js.contains("rows.taxpayer_basis"));
        assert!(js.contains("'Figures assume '"));
        // The reports it is stated on, by the API paths their screens drive.
        assert!(js.contains("/portfolio/parcel-optimiser"));
        assert!(js.contains("/portfolio/realised-gains"));
        // The what-if states it on its scenario rows, listed explicitly there.
        assert!(js.contains("'taxpayer_basis'"));
    }

    #[tokio::test]
    async fn row_history_ui_present() {
        let js = app_js_body().await;
        // The audit-trail screen is one generic params-report entry driving
        // the row-history endpoint (no bespoke view)…
        assert!(js.contains("'row-history'"));
        assert!(js.contains("/reports/row_history"));
        // …whose table picker names every audited table — checked against
        // the Rust const the endpoint validates with, so a table added to
        // the trigger set cannot be forgotten here (an extra/mistyped UI
        // option is caught by the endpoint's own 422).
        for table in crate::reports::row_history::AUDITED_TABLES {
            assert!(
                js.contains(&format!("'{table}'")),
                "table picker missing {table}"
            );
        }
    }

    #[tokio::test]
    async fn timestamps_render_local_with_utc_tooltip() {
        let js = app_js_body().await;
        // RFC 3339 UTC server timestamps (fetched_at, generated_at,
        // uploaded_at, job last-run) are detected by the shared cell renderer…
        assert!(js.contains("isTimestamp"));
        // …displayed in the user's timezone…
        assert!(js.contains("fmtLocalTimestamp"));
        // …with the UTC instant on the cell's hover tooltip (title attr).
        assert!(js.contains("utcTooltip"));
        assert!(js.contains("title: utcTooltip(v)"));
        // The snapshot-detail "Generated …" line gets the same treatment.
        assert!(js.contains("title: utcTooltip(snap.generated_at)"));
    }

    #[tokio::test]
    async fn currency_amounts_round_in_tables_rates_keep_precision() {
        let js = app_js_body().await;
        // Display-only rounding lives in the web layer's formatter, fed by a
        // per-column kind keyed by column name (COLUMN_KINDS) so every table —
        // entity lists, the bespoke Sells/Transfers lists, and the report
        // tables — inherits the rule with no bespoke code.
        assert!(js.contains("COLUMN_KINDS"));
        assert!(js.contains("function columnKinds("));
        assert!(js.contains("const kinds = columnKinds(cols)"));
        // Monetary amounts round to 2 dp (half away from zero) with thousands
        // grouping, via exact BigInt decimal-string arithmetic — never
        // parseFloat on money.
        assert!(js.contains("function roundDecimalStr("));
        assert!(js.contains("function groupThousands("));
        assert!(js.contains("roundDecimalStr(value, 2)"));
        // Representative columns classified: a money amount, a per-unit rate
        // (kept at entered precision), a derived per-unit figure (≥4 dp), and a
        // quantity (kept at entered precision).
        assert!(js.contains("'total_cost_base'"));
        assert!(js.contains("'average_price'"));
        assert!(js.contains("'avg_cost_base_per_unit'"));
        assert!(js.contains("'quantity'"));
        // Rates/quantities keep their precision; derived per-unit figures show
        // at least 4 dp.
        assert!(js.contains("function padMinDp("));
        assert!(js.contains("kind === 'rate4'"));
        // The shared cell renderer applies the kinds and, when money rounding
        // drops precision, keeps the full value on the hover tooltip.
        assert!(js.contains("numericDisplay(row[c], kinds[c])"));
        assert!(js.contains("nd.tip"));
    }

    #[tokio::test]
    async fn listing_management_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/listings"));
        // The preference-share flag (90-day franking holding period) is editable.
        assert!(js.contains("preference"));
        // The Crypto security type is selectable, with the exchange optional
        // (blank for Crypto) — and crypto-aware labels never print "null".
        assert!(js.contains("'Crypto'"));
        assert!(js.contains("l.exchange_mic || 'Crypto'"));
        // SCENARIOS Q-02: the date the price provider stopped quoting the
        // security is enterable, and the hint says what it does.
        assert!(js.contains("unpriced_from"));
        assert!(js.contains("Unpriced from"));
        assert!(js.contains("carries the last stored closing price forward"));
        // …and the mirror: the date its provider series begins, before which
        // the holding leaves the totals (migration 0037).
        assert!(js.contains("unpriced_before"));
        assert!(js.contains("Unpriced before"));
        assert!(js.contains("leaves the holding out of that date\u{2019}s portfolio totals"));
    }

    // SCENARIOS R-01/R-05: the rename endpoint the Listings form's own 422
    // names is reachable from the row it refuses on — one ACTIONS entry,
    // rendered by the generic viewAction like reinvest/exercise/demerge.
    #[tokio::test]
    async fn listing_rename_action_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("slug: 'rename'"));
        assert!(js.contains("'/listings/' + id + '/rename'"));
        // Reached from the listing row itself (and from the chain view).
        assert!(js.contains("{ label: 'Rename', href: '#/rename/' + row.id }"));
        // Every field the endpoint takes, required ones flagged as such.
        assert!(js.contains("dt('effective_date', 'Effective date', { required: true"));
        assert!(js.contains("txt('ticker', 'New ticker', { required: true"));
        assert!(js.contains("fk('exchange_mic', 'New exchange', 'exchanges', { optional: true"));
        assert!(js.contains("txt('name', 'New name', { optional: true"));
        assert!(js.contains("txt('price_symbol', 'New price symbol', { optional: true"));
        assert!(js.contains("txt('note', 'Note', { optional: true"));
        // The screen states the rules the API enforces, so it promises
        // nothing the endpoint refuses: the no-op, the date bounds, the
        // ticker collision, the currency boundary, and the Crypto pairing.
        assert!(js.contains("a no-op is refused"));
        assert!(js.contains("not after today"));
        assert!(js.contains("must not already be held by another listing"));
        assert!(js.contains("exchange quoting a different currency is refused"));
        assert!(js.contains("recognised digital-token ticker"));
        // …and that a takeover relisting is a ScripForScrip action, not this.
        assert!(js.contains("not a rename"));
    }

    // SCENARIOS R-01/R-05: the recorded chain, with the undo the API allows.
    #[tokio::test]
    async fn listing_rename_history_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("viewListingRenames"));
        assert!(js.contains("{ label: 'Rename history', href: '#/renames/' + row.id }"));
        assert!(js.contains("parts[0] === 'renames'"));
        // Reads the chain and undoes an entry through the listing's own paths.
        assert!(js.contains("'/listings/' + pathSeg(listingId) + '/renames'"));
        assert!(js.contains("'/listings/' + pathSeg(listingId) + '/renames/' + row.id"));
        // Rendered through the shared table, with the before/after columns.
        assert!(js.contains("table = filterableTable(rows, cols, {"));
        for col in [
            "'effective_date'",
            "'old_ticker'",
            "'new_ticker'",
            "'old_exchange_mic'",
            "'new_exchange_mic'",
            "'old_name'",
            "'old_price_symbol'",
        ] {
            assert!(js.contains(col), "{col}");
        }
        // Undo is newest-only (the API refuses any other, 422), so it is
        // offered on the newest row alone — by identity, so a re-sort keeps it
        // with the right row.
        assert!(js.contains("const newest = rows[0];"));
        assert!(js.contains("if (row !== newest) return td;"));
        assert!(js.contains("'Undo'"));
        // The before/after exchange columns read like the Exchange column
        // everywhere else, not "Old exchange MIC".
        assert!(js.contains("old_exchange_mic: 'Old exchange'"));
        assert!(js.contains("new_exchange_mic: 'New exchange'"));
    }

    #[tokio::test]
    async fn trade_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/trades"));
    }

    #[tokio::test]
    async fn fk_columns_render_names_not_ids() {
        let js = app_js_body().await;
        // Foreign-key id columns resolve to the referenced row's natural name
        // by *column name* (FK_COLUMN_SOURCES → fkLabelMaps), the one path
        // shared by the generic entity lists and the report tables…
        assert!(js.contains("fkLabelMaps"));
        assert!(js.contains("TABLE_LABEL_SOURCES"));
        assert!(js.contains("FK_COLUMN_SOURCES"));
        assert!(js.contains("columnLabelMaps"));
        // …a listing shows as MIC:TICKER (Crypto listings have no MIC)…
        assert!(js.contains("(l.exchange_mic || 'Crypto') + ':' + l.ticker"));
        // …a holding account shows by its name…
        assert!(js.contains("return a.name;"));
        // …and a trade (an id alone is meaningless) shows side/quantity/
        // listing/date via the shared describeTrade.
        assert!(js.contains("function describeTrade("));
        assert!(js.contains(
            "t.trade_type + ' ' + t.quantity + ' ' + listingName(t.listing_id) + ' on ' + t.date"
        ));
        // The trade/amma id columns are mapped by name — so income's
        // reinvestment_trade_id, the parcel-allocation sale/purchase trade ids,
        // and the amit-adjustment statement/trade ids all render names with no
        // per-entity field config.
        assert!(js.contains("reinvestment_trade_id: 'trades'"));
        assert!(js.contains("sale_trade_id: 'trades', purchase_trade_id: 'trades'"));
        assert!(js.contains("amma_statement_id: 'amma'"));
        // The generic entity list and the report tables both go through it.
        assert!(js.contains("const labels = await columnLabelMaps(cols);"));
        assert!(js.contains("labels: await columnLabelMaps(cols)"));
        // The raw id stays reachable on the cell tooltip.
        assert!(js.contains("'id ' + cellText(v)"));
        // The post-record action pages (Reinvest / Exercise / Participate /
        // Exchange / Demerge) name the listings in their titles and
        // descriptions; the scrip/demerger targets use the resolver.
        assert!(js.contains("action.title(id, owner, listingName)"));
        assert!(js.contains("action.desc(owner, listingName)"));
        assert!(!js.contains("'Creates a DRP trade for listing '"));
        assert!(js.contains("listing(a.scrip_listing_id)"));
        assert!(js.contains("listing(a.demerger_listing_id)"));
        // The allocation-editor parcel options and the AMMA-statement options
        // name the listing too (via the shared resolver).
        assert!(js.contains("listingNamer"));
        assert!(js.contains("listing(t.listing_id)"));
        assert!(js.contains("listing(a.listing_id)"));
        // …and the old raw-id label wording is gone (the "listing N" string
        // survives only as the unknown-id fallback).
        assert!(!js.contains("(listing '"));
        assert!(!js.contains(": listing '"));
    }

    #[tokio::test]
    async fn toasts_and_attachments_name_what_was_created_not_just_an_id() {
        let js = app_js_body().await;
        // Toasts that used to report only a created row's id now name what was
        // created (ticker/quantity/date via describeTrade), with the id as
        // secondary detail — across the action pages and the income form's
        // chained DRP reinvest.
        assert!(js.contains("'Reinvested into ' + describeTrade(trade, listing)"));
        assert!(js.contains("'Exercised into ' + describeTrade(trade, listing)"));
        assert!(js.contains("'Sold into the buy-back: ' + describeTrade(t, listing)"));
        assert!(js.contains("'Saved and reinvested into ' + describeTrade(trade, listingName)"));
        // The buy-back's dividend income and the scrip/demerger closing sells
        // are named by their listing too, not a bare id.
        assert!(js.contains("dividend income for ' + listing(r.income.listing_id)"));
        assert!(js.contains("'Exchanged ' + listing(a.listing_id) + ' into '"));
        assert!(js.contains("'Demerged ' + listing(a.listing_id) + ' into '"));
        // The transfer toast names the listing and both accounts (the
        // transfer-out sell id is only secondary detail).
        assert!(
            js.contains("'Transferred ' + n + ' parcel(s) of ' + listingName(body.listing_id)")
        );
        // The bare "trade #N" toast/heading wording is gone.
        assert!(!js.contains("'Reinvested into trade #'"));
        assert!(!js.contains("'Exercised into trade #'"));
        // The attachments view names the owning activity, not "trade #5".
        assert!(js.contains("const ATTACH_OWNER ="));
        assert!(js.contains("describeTrade(o, listing)"));
        assert!(!js.contains("ATTACH_OWNER_LABEL"));
    }

    #[tokio::test]
    async fn gst_inclusive_brokerage_and_statement_total_ui_present() {
        let js = app_js_body().await;
        // The GST-included checkbox ships on both the trades (Buy/DRP) and
        // Sell forms, driven by the shared wiring helper that hides the GST
        // field and relabels brokerage when ticked…
        assert!(js.contains("brokerage_includes_gst"));
        assert!(js.contains("Brokerage includes GST"));
        assert!(js.contains("wireGstBrokerage"));
        assert!(js.contains("Brokerage (GST-inclusive)"));
        // …and since the API reads a flagged trade's `brokerage` back as the
        // one GST-inclusive amount (the lossless round-trip contract), the
        // form fills the field straight from the row — the old client-side
        // recombination of the split pair is gone (it would double-count the
        // GST on top of the server's re-presentation).
        assert!(!js.contains("addDecimalStrings(existing.brokerage"));
        // The statement-total cross-check field ships on both forms and shows
        // as a column in the trades and Sells lists.
        assert!(js.contains("statement_total"));
        assert!(js.contains("Statement total"));
        assert!(
            js.contains("'statement_total', 'fx_rate'"),
            "trades list column"
        );
        assert!(
            js.contains("'statement_total', 'holding_account_id'"),
            "sells list column"
        );
    }

    /// The deliberate spot-rate override (QC 18020) ships on both the trades
    /// (Buy/DRP) and Sell forms, shows as a trades-list column, and is
    /// display-classified as a rate (full precision, never cent-rounded).
    #[tokio::test]
    async fn spot_fx_rate_override_ui_present() {
        let js = app_js_body().await;
        // Two field declarations: the trades config entry and SELL_FIELDS.
        assert_eq!(js.matches("dec('spot_fx_rate'").count(), 2);
        assert!(js.contains("Spot FX rate override"));
        assert!(js.contains("wins over the monthly RBA rate"));
        // Trades list column, classified as a rate in COLUMN_KINDS.
        assert!(js.contains("'fx_rate', 'spot_fx_rate', 'holding_account_id'"));
        assert!(js.contains("'fx_rate', 'spot_fx_rate', 'amount_per_security'"));
    }

    #[tokio::test]
    async fn income_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/income"));
        // Reinvest-distribution action drives POST /income/:id/reinvest.
        assert!(js.contains("/reinvest"));
        // A reinvested row offers Undo reinvest instead, driving
        // DELETE /income/:id/reinvest through the generic `del` row action
        // (confirm + DELETE + list refresh).
        assert!(js.contains("Undo reinvest"));
        assert!(js.contains("del: '/income/' + row.id + '/reinvest'"));
        assert!(js.contains("await api('DELETE', a.del)"));
    }

    /// An income row states its kind (SCENARIOS J-10): the selector, its
    /// explanation of what an employment-income row may carry, the list column,
    /// and the suppressed Reinvest action — remuneration is not a distribution.
    #[tokio::test]
    async fn income_type_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains(
            "sel('income_type', 'Income type', ['Dividend', 'EmploymentIncome', 'OtherIncome']"
        ));
        assert!(js.contains("not a dividend in your hands (TD 2017/26)"));
        assert!(js.contains("carry the cash as the unfranked amount and nothing else"));
        assert!(js.contains("if (row.income_type && row.income_type !== 'Dividend') return [];"));
        // The third kind: ordinary income at item 24, with its own table in the
        // printed document (SCENARIOS L-03/L-04).
        assert!(js.contains("a crypto staking reward, or an airdrop of an established token"));
        assert!(js.contains("Other income (item 24)"));
        // The simple form can't describe one, so a stored row opens advanced.
        assert!(js.contains("existing.income_type !== 'Dividend'"));
        // The printed annual document gives it its own table and says why.
        assert!(js.contains("Employment income (not investment income)"));
        assert!(js.contains("remuneration under s 6-5, not a dividend "));
        assert!(js.contains("in none of "));
    }

    #[tokio::test]
    async fn income_simple_entry_ui_present() {
        let js = app_js_body().await;
        // The income form opens simple-first: a payment amount + franking
        // selector mapped onto the component body at submit, with the full
        // tax-component field set behind an advanced toggle.
        assert!(js.contains("wireIncomeEntry"));
        assert!(js.contains("simple_amount"));
        assert!(js.contains("simple_franking"));
        assert!(js.contains("Fully franked (30%)"));
        assert!(js.contains("Trust distribution"));
        assert!(js.contains("Show advanced fields"));
        assert!(js.contains("INCOME_ADVANCED_FIELDS"));
        // Fully franked auto-computes the credit at amount × 30/70 with exact
        // BigInt decimal arithmetic (money — no float drift).
        assert!(js.contains("frankingCreditFor"));
        assert!(js.contains("* 3n"));
        // The per-share cross-check pair ships with a live computed-product
        // hint, driving the server-side 422 validation.
        assert!(js.contains("amount_per_security"));
        assert!(js.contains("securities_held"));
        assert!(js.contains("mulToCents"));
        // The DRP tick chains the existing reinvest POST after the save and
        // keeps the income on a reinvest failure.
        assert!(js.contains("Reinvested under DRP"));
        assert!(js.contains("'/income/' + id + '/reinvest'"));
        assert!(js.contains("reinvestment_price"));
        assert!(js.contains("Retry from the row’s Reinvest action"));
        // A fractional broker plan enters the statement's exact units; blank
        // keeps the whole-share default (the field is omitted from the body).
        assert!(js.contains("drp_units"));
        assert!(js.contains("Units allotted (fractional plans)"));
        assert!(js.contains("body.units = drpUnitsInput.value.trim()"));
        // The generic form honours the wireForm hook's submit extensions.
        assert!(js.contains("transformBody"));
        assert!(js.contains("afterSave"));
    }

    #[tokio::test]
    async fn income_entitlement_date_ui_present() {
        let js = app_js_body().await;
        // Trust present-entitlement timing (docs/ato/trust-income-timing.md):
        // the field exists, selecting Trust reveals it in simple mode
        // prefilled with the pay date, and switching away clears it so the
        // server's trust-only 422 can't be tripped by a leftover value.
        assert!(js.contains("entitlement_date"));
        assert!(js.contains("Entitlement date"));
        assert!(js.contains("applyEntitlement"));
        assert!(js.contains("presently entitled"));
        assert!(
            js.contains("if (mode !== 'Trust') { body.entitlement_date = null; body.tax_deferred_amount = null; }")
        );
    }

    #[tokio::test]
    async fn income_tax_deferred_e4_ui_present() {
        let js = app_js_body().await;
        // The tax-deferred amount joins the advanced income fields (CGT event
        // E4, docs/ato/cgt-non-assessable-payments.md) and is cleared when
        // simple mode switches away from Trust (asserted above with the
        // entitlement date); the cross-check report screen drives
        // GET /reports/e4_cross_check.
        assert!(js.contains("tax_deferred_amount"));
        assert!(js.contains("Tax-deferred amount"));
        assert!(js.contains("/reports/e4_cross_check"));
        assert!(js.contains("Tax-Deferred E4 Cross-Check"));
    }

    /// The franking-credit ceiling is enforced server-side, so the form has to
    /// say what the bound is before a user meets the 422 (SCENARIOS G-25) —
    /// including the trust exemption, which is why an identical-looking row
    /// with the trust tick is accepted.
    #[tokio::test]
    async fn income_franking_credit_ceiling_hint_present() {
        let js = app_js_body().await;
        assert!(js.contains("a company can attach at most franked × 30/70"));
        assert!(js.contains("Trust distributions are exempt"));
    }

    /// The conduit-foreign-income entry convention has to reach the user at
    /// the two places they meet the figure (SCENARIOS G-03): the income form,
    /// where a bare "Conduit foreign income" input invited keying the
    /// statement's CFI line as an amount of its own, and the annual tax
    /// report, which now prints it as a memo column headed as one.
    #[tokio::test]
    async fn income_conduit_foreign_income_memo_ui_present() {
        let js = app_js_body().await;
        // The form hint states the convention and the resident's treatment.
        assert!(js.contains("already included in that amount"));
        assert!(js.contains("declared to be conduit foreign income (CFI)"));
        // The tax report prints the memo column, headed so the two figures
        // can't be read as additive, with the note under the dividend table.
        assert!(js.contains("'conduit_foreign_income_aud'"));
        assert!(js.contains("CFI, within unfranked (AUD)"));
        assert!(js.contains("cfiFootnote"));
        assert!(js.contains("not additional to it"));
    }

    #[tokio::test]
    async fn amit_adjustment_cross_check_ui_present() {
        let js = app_js_body().await;
        // The set-level cross-check screen drives GET
        // /reports/amit_adjustment_cross_check: an AMMA statement's per-parcel
        // adjustments are validated row by row at write time, so only this
        // report sees a set that doesn't reconcile to the statement.
        assert!(js.contains("/reports/amit_adjustment_cross_check"));
        assert!(js.contains("AMIT Adjustment Cross-Check"));
        // Its comparison columns are classified, and the problems list renders
        // as sentences rather than String(array)'s comma run-on.
        assert!(js.contains("'units_adjusted'"));
        assert!(js.contains("Array.isArray(v)"));
        // Generation is reachable both as the AMMA form's chain-after-save
        // tick and as the statement row's standing action, and both go through
        // the preview-and-confirm gate before anything is written.
        assert!(js.contains("Generate AMIT adjustments"));
        assert!(js.contains("Generate adjustments"));
        assert!(js.contains("function confirmGeneratedAdjustments"));
        assert!(js.contains("function adjustmentPreviewText"));
        assert!(js.contains("preview: true"));
        assert!(js.contains("Replace existing adjustments"));
        // The annual tax report prints the alerts as a fourth completeness
        // bullet.
        assert!(js.contains("amit_adjustment_alerts"));
    }

    #[tokio::test]
    async fn rollover_consistency_ui_present() {
        let js = app_js_body().await;
        // The cross-check screen drives GET /reports/rollover_consistency: the
        // three parcel-substituting operations store the cost base their
        // replacement parcels carry, so a later change behind one is only
        // visible here (SCENARIOS N-06, N-07).
        assert!(js.contains("/reports/rollover_consistency"));
        assert!(js.contains("Rollover Consistency"));
        // The screen says what the fix is, and what it does not check.
        assert!(js.contains("delete that operation and run it again"));
        assert!(js.contains("not checked"));
        // The annual tax report prints the alerts as a fifth completeness
        // bullet, whatever year the operation was in.
        assert!(js.contains("rollover_alerts"));
    }

    #[tokio::test]
    async fn amit_cash_cross_check_ui_present() {
        let js = app_js_body().await;
        // The cross-check report screen drives GET /reports/amit_cash_cross_check
        // (AMIT cash rows are cash-only — the AMMA is the assessable record —
        // so a missing AMMA must be visible, not silent), and its AUD cash
        // column is classified money with an explicit (AUD) heading.
        assert!(js.contains("/reports/amit_cash_cross_check"));
        assert!(js.contains("AMIT Cash Cross-Check"));
        assert!(js.contains("cash_total_aud"));
        assert!(js.contains("Cash total (AUD)"));
    }

    #[tokio::test]
    async fn interest_income_ui_present() {
        let js = app_js_body().await;
        // The Interest Income CRUD screen drives the /interest_income API,
        // with the gross amount (incl. TFN withheld) and the withheld amount.
        assert!(js.contains("/interest_income"));
        assert!(js.contains("Interest Income"));
        assert!(js.contains("Gross amount"));
        // The foreign-source classification (REQUIREMENTS 2026-07-13): the
        // form carries the flag and the foreign tax withheld field.
        assert!(js.contains("foreign_source"));
        assert!(js.contains("Foreign tax paid"));
        // The tax-summary interest columns (Australian 10L and foreign 20E)
        // are classified as money so they format automatically when the
        // report response carries them.
        assert!(js.contains("interest_income"));
        assert!(js.contains("foreign_interest_income"));
    }

    /// The one date an interest row carries names itself unambiguously
    /// (SCENARIOS H-05): interest is assessed in the year it is **credited**,
    /// so the field is labelled and hinted as the credit date — keying the
    /// date the funds became reachable instead moves a whole year's interest
    /// into the wrong return. The convention it states is pinned against the
    /// ATO wording by `doc_checks::interest_credited_date_convention_documented`.
    #[tokio::test]
    async fn interest_income_date_credited_hint_present() {
        let js = app_js_body().await;
        assert!(js.contains("'date_paid', 'Date credited'"));
        assert!(js.contains("The date the interest was credited, received, or applied on your"));
        assert!(js.contains("not the date the funds became reachable"));
        // The worked case the convention was written for.
        assert!(js.contains("credited 30 June and withdrawable 2 July"));
    }

    #[tokio::test]
    async fn investment_expenses_ui_present() {
        let js = app_js_body().await;
        // The Investment Expenses CRUD screen drives the /investment_expenses API,
        // with the deductible-expense type selector and the deductible amount.
        assert!(js.contains("/investment_expenses"));
        assert!(js.contains("Investment Expenses"));
        assert!(js.contains("expense_type"));
        assert!(js.contains("LoanInterest"));
        assert!(js.contains("ManagementFee"));
        assert!(js.contains("Deductible amount"));
        // The new tax-summary deduction/net columns are classified as money so
        // they format automatically when the report response carries them.
        assert!(js.contains("gross_assessable_investment_income"));
        assert!(js.contains("deductions_total"));
        assert!(js.contains("net_assessable_investment_income"));
        // The per-destination deduction lines (SCENARIOS P-08) format as money
        // too, and carry the question each is claimed at in the heading — the
        // label is the whole point of the column.
        assert!(js.contains(
            "deductions_trust_distributions: 'Deductions, trust distributions 13Y (AUD)'"
        ));
        assert!(js.contains("deductions_foreign_income: 'Deductions, foreign income 20M (AUD)'"));
        assert!(js.contains("deductions_foreign_debt: 'Deductions, foreign debt D15 (AUD)'"));
        assert!(js.contains(
            "deductions_dividend_and_interest: 'Deductions, dividends/interest D7-D8 (AUD)'"
        ));
    }

    /// A multi-year expense has to be keyed one row per year (SCENARIOS H-08),
    /// so the form says so where the year-setting date is entered and the
    /// screen's description says so before the form is reached. The rules and
    /// their ATO sources are pinned by
    /// `doc_checks::multi_year_expense_apportionment_documented`.
    #[tokio::test]
    async fn investment_expense_per_year_entry_hint_present() {
        let js = app_js_body().await;
        assert!(js.contains("One row is one year: an expense spread across years"));
        assert!(js.contains("5 years or the loan term, whichever is shorter"));
        assert!(js.contains("one row per financial year carrying that year"));
        // …and the entity description carries it too.
        assert!(js.contains("is entered as one row per year carrying that year"));
    }

    #[tokio::test]
    async fn amma_statement_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/amma_statements"));
    }

    #[tokio::test]
    async fn ess_statement_ui_present() {
        let js = app_js_body().await;
        // The ESS statement CRUD screen and its discount labels.
        assert!(js.contains("/ess_statements"));
        assert!(js.contains("ESS Statements"));
        assert!(js.contains("taxed_upfront_eligible"));
        assert!(js.contains("deferral_discount"));
        assert!(js.contains("market_value_per_share"));
        // The statement-AUD override fields (the employer's stated AUD figures,
        // reported verbatim by the tax summary when present).
        assert!(js.contains("aud_deferral_discount"));
        assert!(js.contains("aud_taxed_upfront_eligible"));
        assert!(js.contains("Statement AUD: deferral discount (F)"));
        // The statement-AUD deferral figure (label F) is also a list column,
        // money-formatted, with a header the humaniser can't produce.
        assert!(js.contains("'deferral_discount', 'aud_deferral_discount'"));
        assert!(js.contains("Statement AUD deferral (F)"));
        // The Vest action (creates the cost-base-reset Buy) is reachable from a
        // statement row and posts to the vest endpoint.
        assert!(js.contains("#/ess-vest/"));
        assert!(js.contains("/ess_statements/' + id + '/vest"));
        // …but only while unvested: a vested row (vest_trade_id set) offers no
        // Vest action, and the vest_trade_id column names the linked Buy.
        assert!(js.contains("row.vest_trade_id == null"));
        assert!(js.contains("vest_trade_id: 'trades'"));
        // The new tax-summary ESS columns are classified as money so they format.
        assert!(js.contains("ess_discount_assessable"));
    }

    #[tokio::test]
    async fn inheritance_ui_present() {
        let js = app_js_body().await;
        // The inheritance CRUD screen drives /inheritances.
        assert!(js.contains("/inheritances"));
        assert!(js.contains("Inheritances"));
        // The form shows only the chosen cost-base rule's fields: the
        // deceased's acquisition date belongs to the post-CGT rule alone, and
        // the per-rule labels rename the cost-base figure.
        assert!(js.contains("cost_base_rule"));
        assert!(js.contains("DeceasedCostBase"));
        assert!(js.contains("MarketValueAtDeath"));
        assert!(js.contains("deceased_acquisition_date"));
        assert!(js.contains("Deceased’s cost base at death"));
        assert!(js.contains("Market value at death"));
        // The LPR expenditure pair, classified as money so it formats.
        assert!(js.contains("lpr_expenditure"));
        assert!(js.contains("lpr_expenditure_date"));
        assert!(js.contains("'lpr_expenditure'"));
        // The two conventions the cost-base figure carries, where it is typed
        // (SCENARIOS K-02, K-09), and the LPR test beside its own field.
        assert!(js.contains("half the units carry half the deceased\u{2019}s cost base"));
        assert!(js.contains("recalculated out first (QC 66053)"));
        assert!(js.contains("what the LPR incurred administering the estate"));
        assert!(js.contains("Not anything billed before the death"));
        assert!(js.contains("AUD inheritances only"));
        // …and that the FX fallback names the month it would apply to.
        assert!(js.contains("a non-AUD inheritance with no rate either way is refused"));
    }

    #[tokio::test]
    async fn parcel_allocation_ui_present() {
        let js = app_js_body().await;
        // Allocations are entered as part of a Sell (PUT /sells/:id).
        assert!(js.contains("/sells"));
        assert!(js.contains("allocations"));
        // The allocation rows are built by the shared allocationEditor helper,
        // driven by the Sell form, the Transfer form (twice: the parcels to
        // move and the optional crypto network-fee parcels), and the buy-back
        // Participate action: one definition plus four call sites.
        assert!(js.contains("function allocationEditor"));
        assert_eq!(js.matches("allocationEditor(").count(), 5);
    }

    #[tokio::test]
    async fn drp_enrolment_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/drp_enrolments"));
        // The period model: enrol/unenrol/re-enrol via dated periods.
        assert!(js.contains("enrolment_date"));
        assert!(js.contains("unenrolment_date"));
    }

    #[tokio::test]
    async fn holding_account_ui_present() {
        let js = app_js_body().await;
        // The Holding Accounts entity view drives the /holding_accounts CRUD
        // API, and trades/income/AMMA/DRP enrolments (and the Sell and
        // exercise/participate forms) select an account.
        assert!(js.contains("/holding_accounts"));
        assert!(js.contains("holding_account_id"));
        assert!(js.contains("holdingAccounts"));
    }

    #[tokio::test]
    async fn transfers_ui_present() {
        let js = app_js_body().await;
        // The Transfers view lists/creates/deletes holding-account transfers
        // via the /transfers API, with from/to accounts and per-parcel
        // quantities.
        assert!(js.contains("/transfers"));
        assert!(js.contains("from_account_id"));
        assert!(js.contains("to_account_id"));
        assert!(js.contains("viewTransferForm"));
        // The optional crypto network fee: a fee-parcel allocation editor plus
        // the per-unit market value driving the fee's CGT disposal.
        assert!(js.contains("fee_allocations"));
        assert!(js.contains("fee_market_price"));
    }

    #[tokio::test]
    async fn trade_origin_labelling_present() {
        let js = app_js_body().await;
        // The Trades and Sells lists carry a derived Origin column labelling
        // the operation that created each row from its provenance links
        // (tradeOrigin in util.js, wired via the trades config's deriveRow and
        // the Sells view), so a rollover Buy's cost-base-carrying brokerage
        // figure — e.g. a transfer-in of a whole ETH parcel — never reads as
        // a real fee.
        assert!(js.contains("function tradeOrigin"));
        assert!(js.contains("deriveRow: function (row) { row.origin = tradeOrigin(row); }"));
        assert!(js.contains("entity.deriveRow"));
        assert!(js.contains("t.origin = tradeOrigin(t)"));
        assert!(js.contains("brokerage = carried cost base, not a fee"));
    }

    #[tokio::test]
    async fn rights_sales_ui_present() {
        let js = app_js_body().await;
        // The Rights Sales view lists the disposals recorded by the Sell
        // rights action via GET /rights_sales. Rows are operation-created and
        // immutable, so the entity is delete-only: Delete stays as the undo
        // path (DELETE /rights_sales/:id frees the entitlement) but there is
        // no New or Edit form.
        assert!(js.contains("'/rights_sales'"));
        assert!(js.contains("deleteOnly: true"));
        assert!(js.contains("entity.deleteOnly"));
    }

    #[tokio::test]
    async fn portfolio_overview_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/portfolio/overview"));
        // The overview is the app's home screen: shortcut buttons for the
        // most common data-entry paths, reflowed above the performance
        // panel's chart so the headline stats are visible without scrolling.
        assert!(js.contains("report.shortcuts"));
        assert!(js.contains("'#/e/trades/new'"));
        assert!(js.contains("'#/e/income/new'"));
        assert!(js.contains("'#/sells/new'"));
        assert!(js.contains("'#/transfers/new'"));
        assert!(js.contains("statsHolder"));
        assert!(js.contains("summary.headline"));
        assert!(js.contains("summary.detail"));
    }

    #[tokio::test]
    async fn portfolio_overview_range_presets_and_activity_filter_present() {
        let js = app_js_body().await;
        // 2Y/3Y presets alongside the existing 1M/3M/6M/1Y/FY/All.
        assert!(js.contains("RANGE_PRESETS"));
        assert!(js.contains("['2y', '2Y']"));
        assert!(js.contains("['3y', '3Y']"));
        // The chosen preset is remembered across reloads (localStorage), and
        // a custom range clears it rather than being remembered itself.
        assert!(js.contains("share-tracker.overview.range"));
        assert!(js.contains("loadPref"));
        assert!(js.contains("savePref"));
        assert!(js.contains("syncPresetButtons"));
        // The per-holding contributions "hide no-activity holdings" checkbox,
        // default checked, also remembered across reloads.
        assert!(js.contains("share-tracker.overview.hideInactive"));
        assert!(js.contains("holdingHasActivity"));
        assert!(js.contains("Hide holdings with no activity in this period"));
    }

    #[tokio::test]
    async fn top_menu_bar_ui_present() {
        let js = app_js_body().await;
        let css = STYLE_CSS;
        // The top menu bar replaces the old left sidebar: a config-driven
        // model (navModel) grouping ENTITIES/REPORTS by `menu`, with the
        // Reports menu further split into titled sections (a mega-menu,
        // since it holds far more entries than the other three menus).
        assert!(js.contains("function navModel("));
        assert!(js.contains("export const MENUS"));
        for label in ["Activity", "Reports", "Reference Data", "Jobs"] {
            assert!(js.contains(label), "menu label {label} missing");
        }
        for section in [
            "Portfolio",
            "CGT & tax",
            "Decision support",
            "Cross-checks & alerts",
        ] {
            assert!(js.contains(section), "report section {section} missing");
        }
        assert!(js.contains("menu-panel"));
        assert!(js.contains("menu-label"));
        // Panels expand on hover and on keyboard focus (no JS needed to open
        // them), pinned here since no bundle-string assertion touches CSS.
        assert!(css.contains(".menu:hover .menu-panel"));
        assert!(css.contains(".menu:focus-within .menu-panel"));
        // A hovered/active menu label must set its own background — the
        // generic `button:hover` rule's light background is a different
        // property to `.menu-label:hover`'s color change, so without this it
        // still applies and gives near-white text on a near-white button.
        assert!(css.contains(".menu-label:hover, .menu-label.active { color: #fff; background:"));
    }

    /// "Log out" (nav.js) renders in the top bar only when `[auth]` is
    /// configured, as a real form POST — not a fetch() or a hash route — so
    /// it works with no JS beyond building the element and needs no CSRF
    /// token beyond the session cookie's own `SameSite=Lax`.
    #[tokio::test]
    async fn logout_ui_present_only_when_auth_is_configured() {
        let js = app_js_body().await;
        assert!(js.contains("authEnabled()"));
        assert!(js.contains("method: 'post'"));
        assert!(js.contains("action: apiUrl('/logout')"));
        assert!(js.contains("'Log out'"));
    }

    #[tokio::test]
    async fn overview_is_the_home_screen() {
        let js = app_js_body().await;
        // An empty hash renders the overview report directly rather than
        // redirecting via location.hash, so `#/` is a stable home URL.
        assert!(js.contains("return await viewReport(reportBySlug.overview)"));
        assert!(!js.contains("location.hash = '#/r/overview'"));
    }

    #[tokio::test]
    async fn open_parcels_report_ui_present() {
        let js = app_js_body().await;
        // The Open Parcels report view drives GET /portfolio/open-parcels and
        // renders through the shared filterable table like every report.
        assert!(js.contains("/portfolio/open-parcels"));
    }

    #[tokio::test]
    async fn settlement_coverage_report_ui_present() {
        let js = app_js_body().await;
        // The Settlement Holiday Coverage report view drives
        // GET /reports/settlement_holiday_coverage and badges its status field.
        assert!(js.contains("/reports/settlement_holiday_coverage"));
        assert!(js.contains("coverage_status"));
        // SCENARIOS S-05: the report's second question — a stored settlement
        // date that is not a trading day — is a column of its own, labelled
        // rather than left to the humaniser.
        assert!(js.contains("settlement_non_trading_reason"));
        // The third coverage_status value badges neutrally: such a row is
        // listed for the settlement-day question, not for its coverage.
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains(".badge.inside_holiday_coverage"));
        // SCENARIOS S-04: the repair the report points at — the unscheduled
        // recompute job, described in the Jobs view — and, on the Trades
        // screen, which dates it may rewrite (the provenance column and the
        // settlement-date field's hint saying an entered value is kept).
        assert!(js.contains("settlement-recompute"));
        assert!(js.contains("settlement_date_source"));
        assert!(js.contains("a date you enter is kept exactly as given"));
    }

    #[tokio::test]
    async fn wash_sales_report_ui_present() {
        let js = app_js_body().await;
        // The Wash Sales report view drives POST /reports/wash_sales with a
        // configurable window field (blank = the 30-day default).
        assert!(js.contains("'wash-sales'"));
        assert!(js.contains("/reports/wash_sales"));
        assert!(js.contains("window_days"));
    }

    #[tokio::test]
    async fn franking_at_risk_ui_present() {
        let js = app_js_body().await;
        // The Franking At-Risk view drives GET /reports/franking_at_risk and
        // badges its status field; the what-if view drives the
        // contemplated-sale endpoint with listing/date/units params.
        assert!(js.contains("'franking-at-risk'"));
        assert!(js.contains("/reports/franking_at_risk"));
        assert!(js.contains("'franking-what-if'"));
        assert!(js.contains("/reports/franking_at_risk/what-if"));
        // The report's third status is explained where the report is described
        // (SCENARIOS G-11): a row the rule could not be applied to at all.
        assert!(js.contains("untested_no_ex_date"));
        // And the two qualified-person tests that are *not* modelled bound the
        // all-clear the description offers (SCENARIOS G-14), so the screen
        // itself doesn't promise more than the recorded data can support.
        assert!(js.contains(
            "the 30%-at-risk test (hedges, options, futures) and the related payments rule"
        ));
        assert!(
            js.contains(
                "assumes the holdings are unhedged and under no related-payment obligation"
            )
        );
        // Surfaced in the Sell flow: the Sells list and the Sell form link to
        // the foresight reports.
        assert!(js.contains("function sellForesightLinks()"));
        assert!(js.contains("#/r/franking-what-if"));
        assert!(js.contains("#/r/wash-sales"));
    }

    #[tokio::test]
    async fn gains_report_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/portfolio/unrealised-gains"));
        assert!(js.contains("/portfolio/realised-gains"));
        assert!(js.contains("/portfolio/net-capital-gain"));
    }

    #[tokio::test]
    async fn expandable_parcel_detail_ui_present() {
        let js = app_js_body().await;
        // filterableTable's generic expand-to-a-child-table option, and the
        // Expand all / Collapse all control every expandable table gets.
        assert!(js.contains("opts.expand"));
        assert!(js.contains("Expand all"));
        assert!(js.contains("Collapse all"));
        // The Realised Gains report expands each disposal to its `parcels`
        // breakdown; the Net Capital Gain report expands each year to its
        // `disposals`, each of which nests its own `parcels` in turn.
        assert!(js.contains("'parcels'"));
        assert!(js.contains("'disposals'"));
        // The parcel optimiser / pre-sale what-if fold their sibling
        // `allocations` table into the same inline expansion, matched back to
        // the parent row by `matchOn`.
        assert!(js.contains("matchOn"));
        assert!(js.contains("'strategy'"));
        // Both drilldown-bearing reports are still driven by their existing
        // API paths — the feature is additive, not a new endpoint.
        assert!(js.contains("/portfolio/realised-gains"));
        assert!(js.contains("/portfolio/net-capital-gain"));
        // The toggle/detail-row/expand-all styling ships in the bundle too.
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains(".expand-toggle"));
        assert!(css.contains(".expand-all-bar"));
        assert!(css.contains("td.detail-cell"));
    }

    #[tokio::test]
    async fn tax_summary_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/portfolio/tax-summary"));
    }

    /// The Annual Tax Report — a printable per-year document, distinct from
    /// the multi-year Tax Summary screen above. Pins: the config entry routes
    /// through the `custom` dispatch (not the generic `filterableTable`
    /// report machinery — a print document needs neither its pager nor its
    /// filter row), both endpoints, the Generate/Print controls, and every
    /// section heading; the print stylesheet is pinned separately below.
    #[tokio::test]
    async fn annual_tax_report_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("custom: 'tax-report'"));
        assert!(js.contains("/reports/tax-report/years"));
        assert!(js.contains("/reports/tax-report"));
        assert!(js.contains("viewTaxReport"));
        assert!(js.contains("Generate report"));
        assert!(js.contains("Print / Save as PDF"));
        assert!(js.contains("window.print()"));
        assert!(js.contains("Data completeness"));
        assert!(js.contains("Trading activity"));
        assert!(js.contains("Gain / loss summary"));
        assert!(js.contains("Overall tax summary"));
        // The Deductions table names the holding the expense was attributed to
        // (SCENARIOS H-07): without the column the archived PDF carries no
        // trace of the attribution, which is exactly what a rename or demerger
        // makes irrecoverable.
        assert!(js.contains(
            "genericTable(inc.deductions, ['date_incurred', 'expense_type', 'ticker', 'amount_aud', 'ato_label', 'description'])"
        ));
        // …and where each deduction goes on the return (SCENARIOS P-08): the
        // amount alone doesn't say which question it is claimed at, and the
        // destinations span four of them.
        assert!(js.contains("ato_label: 'ATO label'"));
        assert!(js.contains("deductionDestinationFootnote(inc)"));
        for label in ["13Y", "20M", "D15", "D7/D8"] {
            assert!(js.contains(label), "the deductions footnote names {label}");
        }
    }

    /// The AMMA component breakdown is rendered transposed (components down
    /// the page, one column per statement) — one row per statement would need
    /// ~1400px and lose its right-hand components off the printed page. Pins
    /// the transposed renderer, its width-capping class, and that every
    /// component of the row the report returns is listed (the wide layout had
    /// silently dropped `tfn_withholding_tax_aud`; the transpose has room).
    #[tokio::test]
    async fn annual_tax_report_amma_table_is_transposed() {
        let js = app_js_body().await;
        assert!(js.contains("ammaStatementsTable"));
        assert!(js.contains("amma-table"));
        assert!(js.contains("year ended "));
        for component in [
            "australian_interest_aud",
            "australian_dividends_unfranked_aud",
            "franked_dividends_aud",
            "franking_credits_aud",
            "net_rent_aud",
            "foreign_income_aud",
            "foreign_tax_credits_aud",
            "other_income_aud",
            "cgt_discount_gains_aud",
            "cgt_indexation_gains_aud",
            "cgt_other_gains_aud",
            "capital_losses_applied_aud",
            "tfn_withholding_tax_aud",
            "tax_deferred_amount",
            "tax_free_amount",
        ] {
            assert!(
                js.contains(component),
                "AMMA component {component} missing from the report"
            );
        }
        assert!(STYLE_CSS.contains("table.amma-table"));
    }

    /// The print rules. Beyond hiding the chrome: the page is fixed to A4
    /// landscape so an archived PDF doesn't depend on a print-dialog setting,
    /// `#app`'s screen-only `overflow-x: auto` is reset to visible (an overflow
    /// box *clips* when printed, silently losing a wide table's right-hand
    /// columns), and document cells wrap at a reduced size so a wide table
    /// compresses onto the page — except `.num`/`.atomic` cells, which must not
    /// break a money figure, date, quantity or price across two lines.
    ///
    /// WebKit implements neither `@page` descriptor, so Safari prints at the
    /// dialog's own orientation and margins: the portrait media query (a point
    /// smaller, so the 12-column disposal schedule keeps real headroom on the
    /// ~330px-narrower page) and the Print button's Safari hint are what make
    /// that path print every column rather than clip one, and are pinned here.
    #[tokio::test]
    async fn annual_tax_report_print_styles_present() {
        let css = STYLE_CSS;
        assert!(css.contains("@media print"));
        assert!(css.contains(".tax-report-doc"));
        assert!(css.contains("#topbar"));
        assert!(css.contains("size: A4 landscape"));
        assert!(css.contains("#app { overflow: visible"));
        assert!(css.contains("white-space: normal; /* long headers wrap"));
        assert!(css.contains("td:not(.num):not(.atomic) { overflow-wrap: anywhere; }"));
        assert!(css.contains("td.atomic { white-space: nowrap; }"));
        assert!(css.contains("@media print and (orientation: portrait)"));
        let js = app_js_body().await;
        assert!(js.contains("class: 'atomic'"));
        assert!(js.contains("Safari ignores the page-size rule"));
    }

    #[tokio::test]
    async fn listing_activity_report_ui_present() {
        let js = app_js_body().await;
        // The Listing Activity report drives POST /portfolio/activity through
        // the generic params-form + titled-tables report machinery: the
        // chronological ledger, then the final holding summary.
        assert!(js.contains("/portfolio/activity"));
        assert!(js.contains("Listing Activity"));
        assert!(js.contains("'events'"));
        assert!(js.contains("'holdings'"));
        // The ledger's numeric columns are classified/labelled for display.
        assert!(js.contains("'amount_aud'"));
        assert!(js.contains("'units_after'"));
        assert!(js.contains("Amount (AUD)"));
    }

    #[tokio::test]
    async fn performance_report_ui_present() {
        let js = app_js_body().await;
        // The Performance report view drives POST /portfolio/performance with
        // the shared price + as-of-date form.
        assert!(js.contains("/portfolio/performance"));
    }

    #[tokio::test]
    async fn live_valuation_ui_present() {
        let js = app_js_body().await;
        // The price-dependent report views value live by default: the shared
        // POST-report runner sends `live: true`, runs on first load (no manual
        // price entry needed), and treats the price form as overrides.
        assert!(js.contains("live: true"));
        assert!(js.contains("function buildBody()"));
        assert!(js.contains("Manual Price Overrides"));
        assert!(js.contains("Leave blank to value from the live price source"));
        // The per-row as-of times roll up into a freshness "as at …" line, with
        // a count of holdings the live fetch could not value.
        assert!(js.contains("function asAtSummary("));
        assert!(js.contains("Live prices as at "));
        assert!(js.contains("price_as_of"));
        assert!(js.contains("price_unavailable"));
        assert!(js.contains("had no live price"));
        // …and a count of holdings valued at a provisional (fallback-month)
        // FX rate, so a flagged valuation is never silently presented.
        assert!(js.contains("fx_provisional"));
        assert!(js.contains("valued at a provisional FX rate"));
        // The "as at" line styling ships in the bundle.
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains(".hint.as-at"));
    }

    #[tokio::test]
    async fn report_export_ui_present() {
        let js = app_js_body().await;
        // The tax-summary and net-capital-gain report views carry an Export CSV
        // action linking to the report's `<api>/export` download endpoint.
        assert!(js.contains("export: true"));
        assert!(js.contains("'/export'"));
        assert!(js.contains("Export CSV"));
    }

    #[tokio::test]
    async fn attachments_ui_present() {
        let js = app_js_body().await;
        // The attachments view lists/uploads/downloads via the /attachments API,
        // reached from the Trade/Income/AMMA/ESS-statement/Interest-income/
        // Corporate-action row "Attachments" action.
        assert!(js.contains("viewAttachments"));
        assert!(js.contains("/attachments"));
        assert!(js.contains("attachOwner"));
        // Every attachable owner is wired: the entity config carries its owner
        // field and the view can name the owning row.
        for owner in [
            "'trade_id'",
            "'income_id'",
            "'amma_statement_id'",
            "'ess_statement_id'",
            "'interest_income_id'",
            "'corporate_action_id'",
        ] {
            assert!(js.contains(&format!("attachOwner: {owner}")), "{owner}");
        }
        assert!(js.contains("ess_statement_id: { noun: 'ESS statement'"));
        assert!(js.contains("interest_income_id: { noun: 'interest income'"));
        assert!(js.contains("corporate_action_id: { noun: 'corporate action'"));
        // Plain-text records are attachable; the file picker offers .txt.
        assert!(js.contains(".pdf,.png,.jpg,.jpeg,.txt"));
        // The checksum is stored integrity metadata, not a user-facing column.
        assert!(!js.contains("'checksum'"));
    }

    #[tokio::test]
    async fn attachments_trade_view_lists_linked_source_documents() {
        let js = app_js_body().await;
        // A trade's attachments view asks the server to traverse the
        // provenance link (DRP funding distribution / buy-back income row /
        // ESS vest statement) so the source record's paperwork shows up…
        assert!(js.contains("include_linked=true"));
        // …only on trade views — the flag is gated on the owner field.
        assert!(js.contains("ownerField === 'trade_id'"));
        // Linked rows are labelled with their owning record and offer a link
        // to that record's own Attachments view instead of Delete (delete
        // stays with the owner).
        assert!(js.contains("linkedOwner"));
        assert!(js.contains("(linked)"));
        assert!(js.contains("Owner's attachments"));
    }

    #[tokio::test]
    async fn attachments_report_ui_present() {
        let js = app_js_body().await;
        // The whole-portfolio attachments index report: a plain GET report
        // over /reports/attachments…
        assert!(js.contains("slug: 'attachments'"));
        assert!(js.contains("/reports/attachments"));
        // …whose row actions link to Download, an inline View (new tab, via
        // ?disposition=inline), and back to the owning record's own
        // per-owner attachments view.
        assert!(js.contains("disposition=inline"));
        assert!(js.contains("'Download'"));
        assert!(js.contains("'View'"));
        assert!(js.contains("'Record'"));
    }

    #[tokio::test]
    async fn sells_list_has_attachments_action() {
        let js = app_js_body().await;
        // The Sells screen is hand-rendered (viewSellsList) rather than the
        // generic entity list, so it doesn't pick up entity.attachOwner
        // automatically — it wires its own link to the same route the
        // generic list uses for the trades entity (attachOwner: 'trade_id').
        assert!(js.contains("'#/attachments/trade_id/' + row.id"));
    }

    #[tokio::test]
    async fn jobs_ui_present() {
        let js = app_js_body().await;
        // The maintenance view lists and triggers jobs via the /jobs endpoints.
        assert!(js.contains("/jobs"));
        // It also surfaces each job's last run (success/error) from the GET /jobs
        // fields, rendered through the shared filterable table.
        assert!(js.contains("last_finished_at"));
        assert!(js.contains("last_error"));
        // Each job row expands to its stored run history (GET /jobs `runs`),
        // so a flapping job's intermittent failures are diagnosable in the UI.
        assert!(js.contains("j.runs"));
        // The run status is the server's own three-valued field, shown as it
        // stands rather than folded into a success boolean: a run that started
        // and never finished (one in flight, or one a restart interrupted)
        // shows as `running`, not as `ok` and not as `failed`, and `never`
        // still means the job has no recorded run (SCENARIOS T-11).
        assert!(js.contains("j.last_status == null ? 'never' : j.last_status"));
        assert!(js.contains("status: r.status"));
        let css = super::STYLE_CSS;
        assert!(
            css.contains(".badge.running"),
            "the in-flight run status needs its own badge, neither ok nor failed"
        );
        // A deliberately schedule-less job is labelled from GET /jobs' own
        // `trigger` flag, so its `never` status reads as expected rather than
        // as an overdue run (SCENARIOS T-09/schedule).
        assert!(js.contains("j.trigger === 'manual_only' ? 'manual only' : 'scheduled'"));
        assert!(js.contains("'trigger'"));
        // …and when the running scheduler says the job is next due, from
        // GET /jobs' own `next_run_at`. Without it this screen could not say
        // whether a job was still scheduled at all, so a timer that had died
        // went on showing its last successful run for ever
        // (SCENARIOS T-11/T-02/T-12).
        assert!(js.contains("next_run: j.next_run_at || ''"));
        assert!(js.contains("'next_run'"));
    }

    #[tokio::test]
    async fn health_banner_ui_present() {
        let js = app_js_body().await;
        // The cross-view banner is driven by the health/freshness endpoint…
        assert!(js.contains("refreshHealthBanner"));
        assert!(js.contains("/reports/health"));
        // …surfacing stale prices, stale FX, and failed jobs, linking to the
        // Jobs page…
        assert!(js.contains("prices_stale"));
        assert!(js.contains("fx_stale"));
        assert!(js.contains("failed_jobs"));
        assert!(js.contains("'#/jobs'"));
        // …plus the two states no *recorded run* can show, both linking to the
        // same Jobs page: a schedule whose timer has stopped moving its stored
        // next run on, and a run that started and never finished
        // (SCENARIOS T-11/T-02/T-12).
        assert!(js.contains("overdue_jobs"));
        assert!(js.contains("is overdue by"));
        assert!(js.contains("j.overdue_hours"));
        assert!(js.contains("j.next_run_at"));
        assert!(js.contains("j.cron"));
        assert!(js.contains("stalled_jobs"));
        assert!(js.contains("has been running since"));
        assert!(js.contains("j.running_hours"));
        // …plus two listings holding one price series between them — the same
        // close on a long run of consecutive trading days, the only signal a
        // series fetched under the wrong symbol leaves — linking to the screen
        // the borrowed rows are cleared from…
        assert!(js.contains("duplicate_price_series"));
        assert!(js.contains("closed at exactly the same price on"));
        assert!(js.contains("consecutive trading day(s)"));
        assert!(js.contains("d.fetched_days"));
        assert!(js.contains("d.manual_days"));
        assert!(js.contains("d.other_fetched_days"));
        assert!(js.contains("'#/prices'"));
        assert!(js.contains("Open Closing Prices"));
        // …plus duplicated corporate actions (silently compounded, so the
        // strip names the type, ticker, date and ids), linking to the screen
        // the surplus row is deleted from…
        assert!(js.contains("duplicate_actions"));
        assert!(js.contains("each is applied separately"));
        assert!(js.contains("'#/e/corporate_actions'"));
        assert!(js.contains("Open Corporate Actions"));
        // …plus two AMMA statements for one fund-year and holding account
        // (SCENARIOS F-06: every figure counted once per statement), linking
        // to the screen the superseded row is deleted from…
        assert!(js.contains("duplicate_amma_statements"));
        assert!(js.contains("every figure is counted once per statement"));
        assert!(js.contains("'#/e/amma_statements'"));
        assert!(js.contains("Open AMMA Statements"));
        // …plus one distribution entered twice (SCENARIOS G-24: the dividend
        // and its franking credits counted once per row), linking to the
        // screen the duplicate is deleted from…
        assert!(js.contains("duplicate_income"));
        assert!(js.contains("identical income rows of"));
        assert!(js.contains("the dividend and its franking credits are counted once per row"));
        assert!(js.contains("'#/e/income'"));
        assert!(js.contains("Open Income"));
        // …plus the same double-entry on the two listing-less sides of the tax
        // summary (SCENARIOS H-01, H-06: an interest credit or a deductible
        // expense counted once per row), each linking to its own screen…
        assert!(js.contains("duplicate_interest"));
        assert!(js.contains("identical interest rows of"));
        assert!(js.contains("the year’s gross interest counts each row"));
        assert!(js.contains("'#/e/interest_income'"));
        assert!(js.contains("Open Interest Income"));
        assert!(js.contains("duplicate_expenses"));
        assert!(js.contains("identical ' + d.expense_type + ' expenses of"));
        assert!(js.contains("the deduction is claimed once per row"));
        assert!(js.contains("'#/e/investment_expenses'"));
        assert!(js.contains("Open Investment Expenses"));
        // …plus the same double-entry on the employee-share-scheme side
        // (SCENARIOS J-11: the discount assessed and the parcel vested once per
        // statement), linking to the screen the superseded row is deleted from…
        assert!(js.contains("duplicate_ess_statements"));
        assert!(js.contains("identical ESS statements for"));
        assert!(js.contains("the discount is assessed and the parcel vested once per statement"));
        assert!(js.contains("'#/e/ess_statements'"));
        assert!(js.contains("Open ESS Statements"));
        // …and on the deceased-estate side (SCENARIOS K-09: the one duplicate
        // that doubles a holding rather than a year's income)…
        assert!(js.contains("duplicate_inheritances"));
        assert!(js.contains("identical inheritances of"));
        assert!(js.contains("the holding and its cost base are doubled"));
        assert!(js.contains("'#/e/inheritances'"));
        assert!(js.contains("Open Inheritances"));
        // …plus the one alert that is a date pattern rather than a double entry
        // (SCENARIOS J-04: a sale inside the ESS 30-day rule's window), which
        // names the days apart, the statement, and the remedy — and the two
        // financial years only when the rule actually moves the discount.
        assert!(js.contains("ess_30_day_rule"));
        assert!(js.contains("day(s) after the taxing point of statement"));
        assert!(js.contains("the 30-day rule moves the taxing point to the sale date"));
        assert!(js.contains("there is no separate capital gain"));
        assert!(js.contains("Enter the employer’s amended statement over the"));
        assert!(js.contains("d.disposal_tax_year === d.statement_tax_year"));
        // The entry form says the same thing where the taxing point is typed.
        assert!(js.contains("30-day rule: if you sell within 30 days after the taxing point"));
        // …plus a listing whose own currency is not the one its exchange quotes
        // in (SCENARIOS R-01): unpriceable from then on, and uncorrectable in
        // place once it has history, so the strip names both currencies and the
        // remedy, linking to the screen the exchange is fixed on.
        assert!(js.contains("exchange_currency_mismatches"));
        assert!(js.contains("but trades on ' + d.exchange_mic"));
        assert!(js.contains("which quotes in ' + d.exchange_currency"));
        assert!(js.contains("its prices cannot be collected"));
        assert!(js.contains("'#/e/listings'"));
        assert!(js.contains("Open Listings"));
        // …plus a trade dated on a day its own exchange was shut (SCENARIOS
        // S-08): the two hand-entry routes refuse one outright, so the strip
        // exists for the rows a derived path wrote, and names the reason, the
        // exchange and which path it came from.
        assert!(js.contains("non_trading_day_trades"));
        assert!(js.contains("d.reason === 'weekend' ? 'a weekend' : 'a public holiday'"));
        assert!(js.contains("the market was shut"));
        assert!(js.contains("correct it to the day the trade actually executed"));
        assert!(js.contains("'#/e/trades'"));
        assert!(js.contains("Open Trades"));
        // …and refreshes on every route render so it appears on the main views.
        assert!(js.contains("refreshHealthBanner(); // deliberately not awaited"));
        // The strip's host element ships in the page shell with its styles.
        let index = body_string(get("/").await).await;
        assert!(index.contains("id=\"health-banner\""));
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains("#health-banner"));
        // `#health-banner`'s own `display: flex` outranks the UA stylesheet's
        // `[hidden] { display: none }` (an id selector beats an attribute
        // selector), so without this higher-specificity id+attribute rule the
        // `hidden` attribute alone doesn't collapse it — the shell renders it
        // `hidden` until a real problem is found, and it must paint nothing
        // (not an empty coloured strip) until then.
        assert!(css.contains("#health-banner[hidden] { display: none; }"));
    }

    #[tokio::test]
    async fn cgt_settings_ui_present() {
        let js = app_js_body().await;
        // The CGT Settings view edits the opening carried-forward capital loss
        // consumed by the net-capital-gain report.
        assert!(js.contains("/cgt_settings"));
        assert!(js.contains("opening_capital_loss"));
    }

    /// The per-year taxpayer settings screen (SCENARIOS J-02): where an
    /// ineligible year is recorded, and where the printed report's footnote
    /// sends the reader.
    #[tokio::test]
    async fn tax_year_settings_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/tax_year_settings"));
        assert!(js.contains("ess_taxed_upfront_reduction_eligible"));
        assert!(js.contains("Tax Year Settings"));
        // The field says what the flag is for and what an absent row means.
        assert!(js.contains("$180,000 or less"));
        assert!(js.contains("no row at all) applies the reduction as before"));
        // The printed annual document footnotes the condition whenever a
        // reduction was actually applied — the cfiFootnote precedent — and
        // names where to record the other answer.
        assert!(js.contains("function essReductionFootnote"));
        assert!(js.contains("record the year as ineligible under Tax Year Settings"));
    }

    #[tokio::test]
    async fn corporate_actions_ui_present() {
        let js = app_js_body().await;
        // The Corporate Actions view records return-of-capital payments (CGT
        // event G1), share splits/consolidations (TD 2000/10), non-assessable
        // bonus issues, rights issues, and off-market buy-backs against a
        // listing.
        assert!(js.contains("/corporate_actions"));
        assert!(js.contains("ReturnOfCapital"));
        assert!(js.contains("amount_per_unit"));
        // …including the optional record date that fixes entitlement to a
        // payment (parcels bought on or after it are not reduced).
        assert!(js.contains("dt('record_date', 'Record date'"));
        assert!(js.contains("ex-entitlement"));
        assert!(js.contains("ShareSplit"));
        assert!(js.contains("split_new_units"));
        assert!(js.contains("split_old_units"));
        assert!(js.contains("BonusIssue"));
        assert!(js.contains("bonus_units"));
        assert!(js.contains("bonus_held_units"));
        assert!(js.contains("RightsIssue"));
        assert!(js.contains("rights_units"));
        assert!(js.contains("rights_held_units"));
        assert!(js.contains("exercise_price"));
        // A RightsIssue row's Exercise action drives the exercise endpoint
        // (a config-driven ACTIONS entry rendered by viewAction).
        assert!(js.contains("#/exercise/"));
        assert!(js.contains("/exercise"));
        assert!(js.contains("rights_cost"));
        // ...and its Sell rights action drives the sell-rights endpoint, with
        // the anchoring-parcel allocation editor posting `units` rows.
        assert!(js.contains("#/sell-rights/"));
        assert!(js.contains("/sell_rights"));
        assert!(js.contains("proceeds_per_right"));
        assert!(js.contains("qtyField: 'units'"));
        assert!(js.contains("BuyBack"));
        assert!(js.contains("buyback_price"));
        assert!(js.contains("buyback_dividend"));
        assert!(js.contains("buyback_franking_credit"));
        assert!(js.contains("buyback_market_value"));
        // A BuyBack row's Participate action drives the participate endpoint.
        assert!(js.contains("#/participate/"));
        assert!(js.contains("/participate"));
        assert!(js.contains("ScripForScrip"));
        assert!(js.contains("scrip_listing_id"));
        assert!(js.contains("scrip_new_units"));
        assert!(js.contains("scrip_old_units"));
        // The optional cash component (partial rollover, Example 27).
        assert!(js.contains("scrip_cash_per_unit"));
        assert!(js.contains("scrip_market_value"));
        assert!(js.contains("scrip_cash_currency"));
        // A ScripForScrip row's Exchange action drives the exchange endpoint.
        assert!(js.contains("#/scrip-exchange/"));
        assert!(js.contains("/exchange"));
        assert!(js.contains("Demerger"));
        assert!(js.contains("demerger_listing_id"));
        assert!(js.contains("demerger_new_units"));
        assert!(js.contains("demerger_held_units"));
        assert!(js.contains("demerger_cost_base_pct"));
        // A Demerger row's Demerge action drives the demerge endpoint.
        assert!(js.contains("#/demerge/"));
        assert!(js.contains("/demerge"));
        // Worthless / delisted shares (CGT events G3 and C2): the type, its
        // event discriminator, and the Recognise action driving the endpoint.
        assert!(js.contains("WorthlessShares"));
        assert!(js.contains("worthless_event"));
        assert!(js.contains("G3Declaration"));
        assert!(js.contains("C2Cancellation"));
        assert!(js.contains("#/recognise/"));
        assert!(js.contains("/recognise"));
    }

    #[tokio::test]
    async fn post_actions_are_config_driven() {
        let js = app_js_body().await;
        // The five post-record action forms (DRP reinvest, rights exercise,
        // buy-back participate, scrip exchange, demerge) are entries in the
        // ACTIONS config rendered by the one generic viewAction, mirroring how
        // ENTITIES drives viewEntityForm.
        assert!(js.contains("const ACTIONS"));
        assert!(js.contains("function viewAction"));
        for slug in [
            "'reinvest'",
            "'exercise'",
            "'participate'",
            "'scrip-exchange'",
            "'demerge'",
            "'recognise'",
            "'sell-rights'",
            "'generate-adjustments'",
        ] {
            assert!(js.contains(slug), "missing action slug {slug}");
        }
        // Each action's POST endpoint ships in the bundle.
        for endpoint in [
            "/reinvest'",
            "/exercise'",
            "/participate'",
            "/exchange'",
            "/demerge'",
            "/recognise'",
            "/sell_rights'",
            "/generate_adjustments'",
        ] {
            assert!(js.contains(endpoint), "missing action endpoint {endpoint}");
        }
        // The reinvest action form takes a statement's stated units
        // (optional — blank keeps the whole-share default).
        assert!(js.contains("dec('units', 'Units allotted (as stated)'"));
    }

    #[tokio::test]
    async fn corporate_action_form_is_split_by_type() {
        let js = app_js_body().await;
        // The corporate-actions form shows only the chosen action_type's
        // fields: a per-type field-group map (plus a per-type description) on
        // the entity config, re-rendered on type change by the generic entity
        // form. Unchosen types' fields submit as null, as their blank inputs
        // used to.
        assert!(js.contains("typeField: 'action_type'"));
        assert!(js.contains("fieldGroups"));
        assert!(js.contains("typeDescs"));
        // The common date field's label is scoped per type too.
        assert!(js.contains("typeLabels"));
        assert!(js.contains("Payment date"));
        assert!(js.contains("Demerger date"));
        // Values typed into a group survive flipping the type away and back...
        assert!(js.contains("const draft"));
        // ...a stale async render can't clobber a newer selection...
        assert!(js.contains("renderSeq"));
        // ...and editing a row to a different type warns that the saved
        // type's fields clear on save.
        assert!(js.contains("clears the saved"));
        for group in [
            "ReturnOfCapital: [",
            "ShareSplit: [",
            "BonusIssue: [",
            "RightsIssue: [",
            "BuyBack: [",
            "ScripForScrip: [",
            "Demerger: [",
            "WorthlessShares: [",
        ] {
            assert!(js.contains(group), "missing field group {group}");
        }
    }

    #[tokio::test]
    async fn tables_are_filterable_and_sortable() {
        let js = app_js_body().await;
        // Every entity list and report table renders through the shared
        // filterableTable, which adds a per-column filter row (inputs AND
        // together) and click-to-sort column headers. Assert those controls
        // ship in the bundle.
        assert!(js.contains("filterableTable"));
        assert!(js.contains("table-filter"));
        assert!(js.contains("filter-row"));
        assert!(js.contains("sortable"));
    }

    #[tokio::test]
    async fn tables_are_paginated() {
        let js = app_js_body().await;
        // Pagination lives in the shared filterableTable, so every entity list
        // and report table inherits it: a 50-row default page with a prev/next
        // pager + "showing m–n of total" count, only one page in the DOM at once.
        assert!(js.contains("const PAGE_SIZE = 50"));
        assert!(js.contains("function updatePager("));
        assert!(js.contains("showing "));
        assert!(js.contains("‹ Prev"));
        assert!(js.contains("Next ›"));
        // Filtering/sorting build the whole result set (visibleRows); only the
        // current page's slice is put in the DOM.
        assert!(js.contains("vr.slice(start, start + PAGE_SIZE)"));
        assert!(js.contains("pageRows.forEach"));
        // A changed filter re-pages from the first page; the pager hides when
        // the filtered total fits one page.
        assert!(js.contains("page = 0; // a changed filter re-pages"));
        assert!(js.contains("if (total <= PAGE_SIZE) { pager.hidden = true"));
        // The pager styling ships in the bundle too.
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains(".pager"));
        assert!(css.contains(".pager-info"));
    }

    #[tokio::test]
    async fn column_headings_are_human_friendly() {
        let js = app_js_body().await;
        // Every table column header and filter placeholder reads through the
        // shared columnLabel — config-driven overrides over a default humaniser,
        // keyed by column name like COLUMN_KINDS — so the chrome around the data
        // never shows a raw database/JSON field name.
        assert!(js.contains("function humanizeLabel("));
        assert!(js.contains("const COLUMN_LABELS"));
        assert!(js.contains("function columnLabel("));
        // The shared table renderer uses the friendly label for both the header
        // cell and the per-column filter placeholder; the raw column name still
        // drives sorting/filtering, so it must not leak into the header text.
        assert!(js.contains("[columnLabel(c), indicator]"));
        assert!(js.contains("'Filter ' + columnLabel(c)"));
        assert!(!js.contains(", [c, indicator]"));
        assert!(!js.contains("'Filter ' + c +"));
        // The default humaniser drops a trailing "_id" (the cell already shows
        // the referenced row's name) and keeps known acronyms in canonical
        // casing rather than title-casing them to "Aud"/"Fx"/"Drp".
        assert!(js.contains("/_id$/"));
        assert!(js.contains("const LABEL_ACRONYMS"));
        assert!(js.contains("aud: 'AUD'"));
        assert!(js.contains("fx: 'FX'"));
        assert!(js.contains("fito: 'FITO'"));
        // Explicit overrides for the headers the requirement calls out, plus a
        // unit qualifier on an always-AUD report aggregate.
        assert!(js.contains("exchange_mic: 'Exchange'"));
        assert!(js.contains("holding_account_id: 'Account'"));
        assert!(js.contains("'Market value (AUD)'"));
    }
}
