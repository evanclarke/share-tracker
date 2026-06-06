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
    }

    #[tokio::test]
    async fn portfolio_overview_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/portfolio/overview"));
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
