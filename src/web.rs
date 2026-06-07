//! Serves the embedded single-page web frontend.
//!
//! The frontend is plain HTML/CSS/JS with no build step — the three assets are
//! compiled into the binary with `include_str!` (the same approach as
//! `schedule.cron`), so the server has no runtime filesystem dependency and the
//! routes are testable with `oneshot`. The app is a hash-routed SPA that talks
//! to the existing JSON API, so `/` is the only HTML entry point (deep links use
//! `#/...` fragments, which never reach the server) and no SPA fallback route is
//! needed.
use axum::{
    Router,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};
use sqlx::SqlitePool;

const INDEX_HTML: &str = include_str!("web/index.html");
const APP_JS: &str = include_str!("web/app.js");
const STYLE_CSS: &str = include_str!("web/style.css");

/// Routes serving the frontend shell and its static assets. Returns a
/// `Router<SqlitePool>` purely so it merges with the entity/report routers; the
/// handlers are stateless.
pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/", get(index))
        .route("/static/app.js", get(app_js))
        .route("/static/style.css", get(style_css))
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

async fn index() -> Response {
    asset("text/html; charset=utf-8", INDEX_HTML)
}

async fn app_js() -> Response {
    asset("text/javascript; charset=utf-8", APP_JS)
}

async fn style_css() -> Response {
    asset("text/css; charset=utf-8", STYLE_CSS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get(uri: &str) -> Response {
        router()
            .with_state(SqlitePool::connect(":memory:").await.unwrap())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_string(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn index_is_served_as_html() {
        let resp = get("/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let body = body_string(resp).await;
        // The shell loads the SPA assets and provides the mount point + nav.
        assert!(body.contains("/static/app.js"));
        assert!(body.contains("/static/style.css"));
        assert!(body.contains("id=\"app\""));
        assert!(body.contains("id=\"nav\""));
    }

    #[tokio::test]
    async fn app_js_is_served_as_javascript() {
        let resp = get("/static/app.js").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/javascript; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn style_css_is_served_as_css() {
        let resp = get("/static/style.css").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/css; charset=utf-8"
        );
    }

    // Each UI item maps to a view registered in the served app bundle. Without a
    // browser harness these assert the view (and the API endpoint it drives) is
    // present in the shipped JS — the honest, testable limit of an embedded SPA.
    async fn app_js_body() -> String {
        body_string(get("/static/app.js").await).await
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
        // The price-import job is described in the Jobs view.
        assert!(js.contains("price-import"));
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
        // …with stale snapshots badged and regenerable per row.
        assert!(js.contains("m.stale ? 'stale' : 'ok'"));
        assert!(js.contains("Regenerate"));
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
        // The chart styles ship in the bundle too.
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains(".series-chart"));
        assert!(css.contains(".badge.stale"));
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
    }

    #[tokio::test]
    async fn trade_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/trades"));
    }

    #[tokio::test]
    async fn income_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/income"));
        // Reinvest-distribution action drives POST /income/:id/reinvest.
        assert!(js.contains("/reinvest"));
    }

    #[tokio::test]
    async fn amma_statement_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/amma_statements"));
    }

    #[tokio::test]
    async fn parcel_allocation_ui_present() {
        let js = app_js_body().await;
        // Allocations are entered as part of a Sell (PUT /sells/:id).
        assert!(js.contains("/sells"));
        assert!(js.contains("allocations"));
        // The allocation rows are built by the shared allocationEditor helper,
        // driven by the Sell form, the Transfer form, and the buy-back
        // Participate action: one definition plus three call sites.
        assert!(js.contains("function allocationEditor"));
        assert_eq!(js.matches("allocationEditor(").count(), 4);
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
    }

    #[tokio::test]
    async fn portfolio_overview_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/portfolio/overview"));
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
    }

    #[tokio::test]
    async fn gains_report_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/portfolio/unrealised-gains"));
        assert!(js.contains("/portfolio/realised-gains"));
        assert!(js.contains("/portfolio/net-capital-gain"));
    }

    #[tokio::test]
    async fn tax_summary_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/portfolio/tax-summary"));
    }

    #[tokio::test]
    async fn performance_report_ui_present() {
        let js = app_js_body().await;
        // The Performance report view drives POST /portfolio/performance with
        // the shared price + as-of-date form.
        assert!(js.contains("/portfolio/performance"));
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
        // reached from the Trade/Income/AMMA row "Attachments" action.
        assert!(js.contains("viewAttachments"));
        assert!(js.contains("/attachments"));
        assert!(js.contains("attachOwner"));
    }

    #[tokio::test]
    async fn jobs_ui_present() {
        let js = app_js_body().await;
        // The maintenance view lists and triggers jobs via the /jobs endpoints.
        assert!(js.contains("/jobs"));
        // It also surfaces each job's last run (success/error) from the GET /jobs
        // fields, rendered through the shared filterable table.
        assert!(js.contains("last_finished_at"));
        assert!(js.contains("last_success"));
        assert!(js.contains("last_error"));
    }

    #[tokio::test]
    async fn cgt_settings_ui_present() {
        let js = app_js_body().await;
        // The CGT Settings view edits the opening carried-forward capital loss
        // consumed by the net-capital-gain report.
        assert!(js.contains("/cgt_settings"));
        assert!(js.contains("opening_capital_loss"));
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
        ] {
            assert!(js.contains(endpoint), "missing action endpoint {endpoint}");
        }
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
}
