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
    }

    #[tokio::test]
    async fn listing_management_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/listings"));
        // The preference-share flag (90-day franking holding period) is editable.
        assert!(js.contains("preference"));
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
        // A RightsIssue row's Exercise action drives the exercise endpoint.
        assert!(js.contains("viewExercise"));
        assert!(js.contains("/exercise"));
        assert!(js.contains("rights_cost"));
        assert!(js.contains("BuyBack"));
        assert!(js.contains("buyback_price"));
        assert!(js.contains("buyback_dividend"));
        assert!(js.contains("buyback_franking_credit"));
        assert!(js.contains("buyback_market_value"));
        // A BuyBack row's Participate action drives the participate endpoint.
        assert!(js.contains("viewParticipate"));
        assert!(js.contains("/participate"));
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
